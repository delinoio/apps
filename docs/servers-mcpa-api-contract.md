# Managed CPA API server contract

## Scope

`servers/mcpa-api` owns the Managed CPA API server, `@servers/mcpa-api`. It covers these parts:

- The Connect management service `mcpa.v1.McpaService`.
- Console authentication through a backend-for-frontend (BFF) cookie session.
- Streamable HTTP MCP and its OAuth broker.
- The embedded CLIProxyAPI data plane that serves model calls under `/r/{resource_id}/`.
- Byte metering, the Delcore usage outbox, budgets, and the retention and tenant-lifecycle workers.
- Explicit SQL migrations, the OCI image, the Delcore catalog example under `servers/mcpa-api/catalog/` and the reusable operator assets under `servers/mcpa-api/ops/`.

The public origin is `https://mcpa.api.delino.io`. All repository-owned files under `servers/mcpa-api` use FSL-1.1-ALv2.

The [protocol contract](protos-mcpa-v1-contract.md) owns the wire messages. The [web console contract](apps-mcpa-web-contract.md) owns the console. This contract owns the behavior behind both. It defines required behavior, not a deployed service. Production deployment, live provider accounts and numeric SLO, latency or capacity targets are outside this contract.

## Runtime and Language

The server uses Go. This deviates from the repository Rust default for new servers because the server embeds the Go [CLIProxyAPI SDK](https://github.com/router-for-me/CLIProxyAPI/blob/main/docs/sdk-usage.md) (`github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy`, MIT), which has no Rust equivalent.

- The server is a package tree inside the root `github.com/delinoio/apps` Go module: package `main` at `servers/mcpa-api/main.go` and internal packages under `servers/mcpa-api/internal/`. It consumes `protos/mcpa/v1` and `protos/delcore/v1` directly from the same module. It adds no separate Go module, no `go.work` and no local `replace` directive. New dependencies go in the root `go.mod` and `go.sum`. The Go toolchain version comes from the root `go.mod`.
- It is not a member of the server Cargo workspace and does not use `servers/crates/`. The shared Rust build task does not build it.
- It imports only the SDK's public `sdk/...` packages, never its `internal/` packages.
- One process embeds the SDK. There is no CPA server or process per resource and no action to create, start or stop a dedicated CPA server.
- PostgreSQL is the only authoritative store. Connect-Go and Protobuf use the root pinned versions.

The binary is `mcpa-api`, built from `./servers/mcpa-api`. It has these subcommands:

| Subcommand                   | Behavior                                                                                                                                                      |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `mcpa-api serve [--dev-env]` | Serve all HTTP surfaces and run the background workers. `--dev-env` loads `servers/mcpa-api/.env` when present; process variables, including empty ones, win. |
| `mcpa-api migrate`           | Apply the embedded SQL migrations explicitly.                                                                                                                 |
| `mcpa-api check-schema`      | Exit non-zero when the database schema differs from the binary's expected version.                                                                            |
| `mcpa-api rotate-master-key` | Rewrap every stored data key under the active master key, in batches, without downtime.                                                                       |

## Users and Operators

- **Resource managers** hold the `manage` role on a resource. They change settings, credentials, routing, ACL, budget and call keys, and view usage, errors and audit.
- **Resource invokers** hold the `invoke` role. They issue and manage their own call keys and view usage.
- **Tenant budget holders** hold the Delcore `manage_tenant_budget` capability and set the tenant budget.
- **Automation** uses Delcore personal service keys (`dlk_`) with Managed CPA capabilities for non-interactive management.
- **Applications** call models with Managed CPA call keys.
- **MCP clients** act for an interactive user through the OAuth broker.
- **Operators** register the Delcore service, meter and price, configure the master keys and the runtime, run migrations, rotate master keys and respond to alerts. Server rollout belongs to private operator automation.

## Interfaces and Contracts

### HTTP surfaces

All surfaces are served from the public API origin.

| Path                                                                               | Purpose                                                            | Authentication                                                                                          |
| ---------------------------------------------------------------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------- |
| `/mcpa.v1.McpaService/<Method>`                                                    | Connect management                                                 | Console session cookie plus `X-Mcpa-CSRF`, or `Authorization: Bearer` with an MCP token or a `dlk_` key |
| `/auth/login?return_to=`                                                           | Start console login (Logto authorization code with PKCE and state) | None                                                                                                    |
| `/auth/callback`                                                                   | Complete console login and set the session cookie                  | None                                                                                                    |
| `/auth/session`                                                                    | `GET` the current session                                          | Session cookie                                                                                          |
| `/auth/refresh`                                                                    | `POST` refresh the session tokens                                  | Session cookie plus CSRF                                                                                |
| `/auth/logout`                                                                     | `POST` end the session                                             | Session cookie plus CSRF                                                                                |
| `/mcp`                                                                             | Streamable HTTP MCP                                                | `Authorization: Bearer` with an MCP-audience token or a `dlk_` key                                      |
| `/.well-known/oauth-protected-resource`, `/.well-known/oauth-authorization-server` | MCP authorization metadata                                         | None                                                                                                    |
| `/oauth/authorize`, `/oauth/callback`, `/oauth/token`, `/oauth/revoke`             | OAuth broker for MCP clients                                       | As defined by OAuth 2.1                                                                                 |
| `/r/{resource_id}/...`                                                             | Model calls (metered)                                              | Call key                                                                                                |
| `/healthz`, `/readyz`, `/metrics`                                                  | Liveness, readiness and Prometheus metrics                         | `/metrics` requires `Authorization: Bearer <MCPA_METRICS_TOKEN>` when that setting is present           |

Only the `/r/{resource_id}/...` surface is metered. Management, console, authentication, OAuth, MCP and operational traffic is free and creates no billing event.

Every response carries `X-Mcpa-Request-Id`, including the 404 and 405 responses that the router generates before a route runs. The value is assigned before routing and stays the same for the whole request. On the call surface its value is the UUID v7 call record ID. Connect errors also carry the `trace_id` in `mcpa.v1.ErrorDetail`.

### Console session and CSRF

The console uses a BFF cookie session that follows the Lane browser-session pattern.

- `/auth/login` accepts `return_to`, which must exactly equal one entry of `MCPA_WEB_RETURN_URLS`, with no query or fragment. Any other value fails before an authentication flow starts. The server redirects to Logto with S256 PKCE and a one-use state value. It requests the `MCPA_API_RESOURCE` audience and the Delcore resource (`DELCORE_RESOURCE`), using the Logto web application `MCPA_LOGTO_CLIENT_ID`.
- `/auth/callback` validates state and PKCE, revalidates `return_to` by equality and exchanges the code. It verifies the MFA evidence of the `MCPA_API_RESOURCE` access token and stores the Logto tokens encrypted server-side. It then sets the session cookie and redirects to `return_to`.
- The session cookie is `mcpa_session`: opaque, `HttpOnly`, `SameSite=Lax`, `Path=/` and `Secure`. Only loopback development omits `Secure`.
- `GET /auth/session` returns 200 `{"user_id","csrf_token","expires_at"}`, where `user_id` is the Delcore internal user ID. Without a valid session it returns 401 `{"error":"unauthenticated"}`. When the token has no MFA evidence it returns 403 `{"error":"mfa_required","reauthentication_url","csrf_token"}`. That CSRF token permits only `POST /auth/logout`, because Connect calls and `POST /auth/refresh` reject a session without MFA evidence.
- `POST /auth/refresh` refreshes the Logto tokens. A refreshed token without MFA evidence ends the session and returns the 403 `mfa_required` response. When Logto rotates the refresh token and a later step of the refresh fails, the server stores the rotated refresh token before it returns the failure, so a retry after recovery does not require a new sign-in. This applies to every session refresh. Once the refresh holds the session lock, it and its commit run detached from the request: a request that is canceled after Logto rotated the refresh token (for example a disconnected client) still stores the replacement, so the next refresh succeeds and the session is not lost. The detached work ends after four times `DEPENDENCY_TIMEOUT`, waiting for the lock stays bound to the request, and the server logs a warning with `request_cancelled=true` and no token material. When Logto rejects the refresh token with `invalid_grant`, the server deletes the session before it returns 401, so the session cookie cannot be used again.
- `POST /auth/logout` deletes the server session and its stored tokens and clears the cookie. It checks `Origin` and CSRF but never refreshes the session tokens, so sign-out works while Logto is unavailable.
- Mutations under `/auth/*` and all Connect calls with a cookie session require `X-Mcpa-CSRF` equal to the session CSRF token. When `Origin` is present it must equal `WEB_ORIGIN`. A rejected request runs no handler.
- CORS allows only `WEB_ORIGIN`, with credentials, for `/auth/*` and Connect, and exposes the Connect response headers and `X-Mcpa-Request-Id`. The call surface and `/mcp` grant no credentialed CORS access.

### MFA evidence

Console and interactive MCP access require Logto MFA evidence in the access token itself.

- Logto does not emit `amr` by default. The operator installs a Logto custom access-token claims script. The script sets the boolean claim named by `MCPA_MFA_CLAIM` (default `mcpa_mfa`) to `true` when the sign-in interaction's verification records contain a verified `Totp`, `WebAuthn` or `BackupCode` record.
- The server accepts either that claim with value `true` or a standard `amr` array that contains `mfa`, `otp` or `hwk`.
- A console or interactive MCP token without evidence is rejected with `ERROR_REASON_MFA_REQUIRED` and `reauthentication_url` set to `MCPA_MFA_REAUTH_URL`. On HTTP surfaces the rejection is 403 `{"error":"mfa_required","reauthentication_url":"..."}`. The console or MCP client sends the user to that URL to enroll in MFA or to reauthenticate.
- A refreshed token that lost the evidence requires reauthentication. The server never caches a positive MFA result beyond the token that carried it.
- A console request admitted with a session that still carried evidence can refresh the Delcore token before the request finishes. When that refresh returns a token without evidence, the session is downgraded and the request fails with `ERROR_REASON_MFA_REQUIRED` and the `MCPA_MFA_REAUTH_URL` reauthentication URL, not `ERROR_REASON_UNAUTHENTICATED`. `GetViewer`, `ListTenantGroups`, `ListResources` and per-resource reads return no data on that request, and a stale group snapshot is not served in its place. Later `GET /auth/session` calls return the 403 `mfa_required` response with a CSRF token that permits logout. A refresh that Logto rejects with `invalid_grant` still deletes the session and returns `ERROR_REASON_UNAUTHENTICATED`. A fresh group snapshot needs no Delcore token, so a request that reads only it is served, and the next request is rejected. Interactive MCP requests keep their grant-scoped behavior: a grant refresh without evidence fails and revokes the grant.
- Delcore personal keys are exempt from MFA.

### MCP and the OAuth broker

`/mcp` serves Streamable HTTP MCP and follows the Lane remote MCP pattern.

- `/.well-known/oauth-protected-resource` names `MCPA_MCP_RESOURCE` as the resource and the API origin as its authorization server. `/.well-known/oauth-authorization-server` publishes the broker endpoints, `S256` as the only PKCE method, the `authorization_code` and `refresh_token` grants and token revocation. An unauthenticated `/mcp` request returns 401 with a `WWW-Authenticate` header that points to the protected-resource metadata.
- `MCPA_OAUTH_CLIENTS` is the operator-approved registry of MCP clients. Each entry binds a client ID to a display name and its exact redirect URIs. There is no dynamic client registration.
- `GET /oauth/authorize` requires a registered client, an exactly matching redirect URI, an S256 code challenge and state. The state must be 1 to 512 bytes, all printable ASCII (`%x20-7E`); an invalid non-empty state is answered with 400 `invalid_request` (`state is invalid`) without a redirect or echo and stores nothing, while a missing state redirects with `invalid_request`. The broker starts its own Logto authorization code flow with PKCE that requests the MCP-only audience `MCPA_MCP_RESOURCE` and the Delcore resource. `/oauth/callback` completes that flow and verifies MFA evidence. It resolves the Delcore internal user ID through `GetCurrentUser` with the Delcore-audience token and refreshes the group snapshots of the user's tenants, bounded as a whole by `DEPENDENCY_TIMEOUT`; a slow or failed refresh never fails the login. It then binds a durable broker grant to that user and client and stores the authorization code, whose lifetime starts at that moment so a slow refresh cannot spend it. Then it redirects to the client's redirect URI with a one-use authorization code and the original state. Cancellation or failure redirects with an OAuth error and the state and issues no code.
- `POST /oauth/token` accepts form-encoded `authorization_code` grants with the PKCE verifier and `refresh_token` grants. Only an exchange that issues tokens consumes the authorization code. When an exchange fails without issuing tokens, for example with `temporarily_unavailable`, the client retries with the same code until it expires. A database failure while the exchange reads or updates the grant, and a Logto outage or rate limit (HTTP 429 or 5xx) on the upstream refresh, answer 503 `temporarily_unavailable` and consume nothing; only a failure of the broker itself, such as sealing or opening the stored tokens, answers 500 `server_error`. A second exchange of a code that issued tokens revokes the grant; of concurrent exchanges of one code, at most one issues tokens. The access token is a Logto-issued token with audience `MCPA_MCP_RESOURCE`. The refresh token is an opaque broker handle. The upstream Logto refresh token stays encrypted on the server. The broker uses it to obtain new MCP-audience tokens and the Delcore-audience tokens that user-token Delcore calls need. A refresh that yields a token without MFA evidence fails and revokes the grant. When Logto rotates the upstream refresh token but the refreshed access token cannot be verified, the endpoint answers 503 with `temporarily_unavailable` and issues no token. The broker still stores the replacement upstream refresh token and keeps the presented refresh handle active, so the client retries with the same handle. The same detached rule applies to the grant: once the grant lock is held, the upstream refresh, the stored replacement token and the commit are not canceled with the request, so a client that disconnects after Logto rotated the upstream refresh token cannot lose it. When the request has ended before the exchange would issue tokens, the broker stores only the rotated upstream refresh token and keeps the presented code or handle active, because the departed client cannot receive a new handle.
- `POST /oauth/revoke` revokes the broker grant and its upstream refresh token. It answers 200 only after the broker grant is deleted, or when the token is unknown. When the grant cannot be deleted, the endpoint answers 503 with `temporarily_unavailable`; the token stays valid and the client retries the revocation. Upstream revocation is best effort and does not change the response.
- `/mcp` verifies each bearer token's issuer, signature, expiry, audience `MCPA_MCP_RESOURCE` and MFA evidence. An MCP-audience token is never forwarded to Delcore, and a console-audience token is not accepted on `/mcp`.
- `/mcp` also accepts a Delcore personal key (`Authorization: Bearer dlk_...`) for non-interactive automation, with the rules in [authorization](#authorization).
- MCP exposes every management feature as a tool. There is one tool for each `McpaService` RPC, named with the snake_case RPC name (for example `create_resource` and `list_call_records`). Its input and output schemas are the ProtoJSON forms of the request and response messages. Tools call the same handlers and authorization as Connect. Tool annotations follow the MCP `ToolAnnotations` semantics and are set per RPC from its handler side effects, not from its name: read-only tools change no stored state; non-destructive tools only add state, which includes `get_provider_o_auth_flow` because a device-code poll can register a credential; tools that overwrite or clear settings (`update_resource`, `update_routing`, `put_acl_entry`, `set_resource_budget` and `set_tenant_budget`) or remove or revoke state are destructive. A failed tool returns an MCP tool error result that carries the `ErrorReason` name, the safe message, the trace ID and, for `ERROR_REASON_MFA_REQUIRED`, the reauthentication URL.

### Delcore integration

Delcore stays service-neutral. This server never changes `servers/delcore-api` or `protos/delcore/v1` and calls Delcore only through its Connect API.

Operators register the service with `PutService`. The service key is `mcpa`, with a Logto machine-to-machine application, the scopes `authorize`, `usage:write` and `usage:read`, and these capabilities:

| Capability             | Purpose                                                                                |
| ---------------------- | -------------------------------------------------------------------------------------- |
| `create_resources`     | Create resources in the tenant.                                                        |
| `access_resources`     | Required, together with a per-resource role, for every per-resource management action. |
| `manage_tenant_budget` | Read and set the tenant budget.                                                        |
| `invoke_models`        | Required for the call key's issuer at model-call time.                                 |

Delcore groups grant these capabilities. The service UUID v7 is configured as `MCPA_SERVICE_ID`.

`ListMeterAggregates` supplies the billing period for every budget read and model-call budget check. Delcore permits it for a service only when both the service registration and the issued token hold `usage:read`. Without that scope, budget RPCs fail and calls on resources or tenants with a budget fail closed.

Operators also register the traffic meter and price:

- Meter: `PutMeter` with key `mcpa-traffic-bytes`, event type `mcpa.traffic`, aggregation `sum` of the numeric attribute `bytes`, grouped by `resource_id`.
- Price: `CreatePrice` with price key `mcpa-traffic`, a USD usage unit price per byte of `0.00000000006519258022308349609375` (exactly USD 0.07 / 2^30, which is USD 0.07 per GiB), zero included usage and no per-request fee.
- `servers/mcpa-api/catalog/` holds a split-file catalog example in the format of `servers/delcore-api/catalog` for development and operators. Production registration stays outside Git.

The server makes these calls:

- Machine-to-machine calls, with the token from `LOGTO_TOKEN_URL`, `CLIENT_ID` and `CLIENT_SECRET` for audience `DELCORE_RESOURCE`: `Authorize`, `AuthorizeApiKey`, `GetTenantStates`, `CheckTenantLifecycle`, `IngestEvent` and `ListMeterAggregates`. `ListMeterAggregates` for meter `mcpa-traffic-bytes` also supplies the bounds of the tenant's current Delcore billing period for budgets.
- User-token calls, only for console and MCP sessions, with the Delcore-audience token from the same Logto login: `GetCurrentUser`, following every `next_after_id` page, for the internal user ID and the user's tenants; and `ListGroups` for group names and for the group-membership snapshot.

Management actions call `Authorize(tenant_id, user_id, capability, paid=false)`. Model calls call `Authorize(tenant_id, issuer_user_id, "invoke_models", paid=true)` and require `GetTenantStates` to report `TENANT_STATE_ACTIVE`.

Authorization cache:

- The server caches only allowed `Authorize` and `AuthorizeApiKey` results, keyed by tenant, user or key, capability and `paid`, and `ACTIVE` tenant states. It caches them for `MCPA_AUTHORIZE_CACHE_TTL`, which the operator must configure.
- It never caches denials, non-`ACTIVE` states or errors.
- After an entry expires, a Delcore error or timeout fails closed: `ERROR_REASON_DEPENDENCY_UNAVAILABLE` for management, and the typed `dependency_unavailable` rejection on the call surface. There is no stale fallback.

Group-membership snapshot:

- Delcore offers registered services no machine-to-machine read of group membership. The server therefore stores a per-tenant snapshot of group IDs, names and member user IDs.
- The snapshot is refreshed from `ListGroups` whenever a console or MCP user session operates in that tenant, at most once per `MCPA_AUTHORIZE_CACHE_TTL`.
- `ListTenantGroups` uses the same refresh, so opening the ACL editor repeatedly does not call `ListGroups` again within the TTL. The group list can therefore lag a newly created Delcore group by up to one `MCPA_AUTHORIZE_CACHE_TTL`. A personal key reads the stored snapshot and never refreshes it.
- Refreshes of one tenant are serialized across replicas with a transaction-scoped PostgreSQL advisory lock. A caller that waited for the lock reads the refresh time again and skips Delcore when another caller refreshed the snapshot. The wait, the `ListGroups` call and the snapshot write together are bounded by `DEPENDENCY_TIMEOUT`, which also bounds how long the transaction holds the lock.
- A failed refresh keeps the previous snapshot and its refresh time. `ListTenantGroups` then serves the previous snapshot and logs the failure. It reports the failure when the tenant has no snapshot yet or when Delcore rejected the user's token.
- Group ACL entries resolve through the latest snapshot. Delcore capability checks stay live. See [known limits](#known-limits).

### Authorization

Every request resolves to an actor kind (`ACTOR_KIND_CONSOLE`, `ACTOR_KIND_MCP` or `ACTOR_KIND_PERSONAL_KEY`) and a Delcore internal user ID. The authorization model has these rules:

- **Per-resource role.** A resource manager assigns Delcore users or groups one of two roles. `RESOURCE_ROLE_MANAGE` permits settings, credentials, routing, ACL, budgets, all call keys, usage, errors and audit. `RESOURCE_ROLE_INVOKE` permits issuing and managing the caller's own call keys and viewing usage. The effective role is the highest of the user's direct entry and the entries of groups that the user belongs to in the snapshot. `MANAGE` includes every `INVOKE` action.
- **Double check.** Every per-resource management action requires the Delcore capability `access_resources` and the required role. Resource creation requires `create_resources`. The creator receives a direct `MANAGE` entry in the same transaction that creates the resource. Tenant budget actions require `manage_tenant_budget`. A per-resource mutation rechecks the role in its transaction after it locks the resource, so an ACL change that commits first applies to a pending mutation.
- **Last manager.** Removing or downgrading the last direct `MANAGE` user entry fails with `ERROR_REASON_FAILED_PRECONDITION`, unless a group holds `MANAGE`.
- **Visibility.** A caller with no role on a resource receives `ERROR_REASON_NOT_FOUND` for that resource, so its existence is not disclosed. A caller with `INVOKE` who attempts a `MANAGE` action receives `ERROR_REASON_PERMISSION_DENIED`. A Delcore capability denial returns `ERROR_REASON_PERMISSION_DENIED`.
- **Management tenant state.** Management uses `paid=false` and does not require an `ACTIVE` tenant. Customers of `BLOCKED` or `DELETING` tenants can still view records and remove credentials and keys, but their model calls are denied. This is the management exception that the Delcore consumer contract permits. Tenants that Delcore reports as `DELETED` or `UNKNOWN` are not served. Delcore authorizes personal keys with a strict tenant lifecycle, so it can deny the unpaid check of a key in a `DELETING` tenant with the single reason `BLOCK_REASON_DELETING`. The server admits that denial for management actions when the decision's tenant and user match the key's attribution. Any other reason, such as `BLOCK_REASON_PERMISSION` alone or next to `BLOCK_REASON_DELETING`, and any denial without reasons stay `ERROR_REASON_PERMISSION_DENIED`. Paid checks, which model calls use, are unchanged.

| Action                                                                                                                                     | Requirement                                                |
| ------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------- |
| Get or list a resource and its endpoints; usage summary and rows for that resource                                                         | `INVOKE`                                                   |
| Issue a call key; list and revoke the caller's own call keys                                                                               | `INVOKE`                                                   |
| Update or delete a resource; routing; ACL; credentials; provider OAuth flows; all call keys; resource budget; call records (errors); audit | `MANAGE`                                                   |
| Create a resource                                                                                                                          | `create_resources`                                         |
| Get or set the tenant budget; tenant-level audit events                                                                                    | `manage_tenant_budget`                                     |
| Tenant-wide usage                                                                                                                          | Sum over resources where the caller has `INVOKE` or higher |

**Personal keys.** A management or MCP request with `Authorization: Bearer dlk_...` calls `AuthorizeApiKey(secret, capability, paid=false)` with the capability of the action.

- A key whose `owner_kind` is `API_KEY_OWNER_KIND_TENANT` is rejected with `ERROR_REASON_TENANT_SERVICE_KEY_REJECTED`.
- A personal key acts as its owner `user_id` inside its attributed `tenant_id` only, with that user's per-resource ACL. Its grant must include the capability.
- Delcore's invalid-secret response maps to `ERROR_REASON_UNAUTHENTICATED`.
- Personal keys are exempt from MFA.
- The server never stores, logs or forwards the secret except to `AuthorizeApiKey`.

### Resources and endpoints

- A resource has a UUID v7 ID, its tenant, a name, a description, routing settings, its creator and timestamps. Names are 1 to 100 bytes, not blank, and unique among the tenant's live resources (`ERROR_REASON_ALREADY_EXISTS`). Descriptions are at most 1,000 bytes.
- `CreateResource` and `IssueCallKey` take a client-generated UUID v7 `request_key`. A replay with the same input within 30 days returns the original result. A replay with different input fails with `ERROR_REASON_IDEMPOTENCY_CONFLICT`. Replays do not extend the 30-day window. A `CreateResource` replay returns the resource's current state with the caller's current role; a caller who no longer holds a role on the resource receives `ERROR_REASON_NOT_FOUND`.
- Each resource reports one `Endpoint` per supported format, derived from `MCPA_PUBLIC_URL`:

| Format | Base URL               | Routes under the resource prefix                                                                                                     |
| ------ | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------ |
| OpenAI | `…/r/{resource_id}/v1` | `/v1/chat/completions`, `/v1/completions`, `/v1/responses` (HTTP only), `/v1/models`                                                 |
| Claude | `…/r/{resource_id}`    | `/v1/messages`, `/v1/messages/count_tokens`, `/v1/models` (Claude-style when the request carries `anthropic-version`, as in the SDK) |
| Gemini | `…/r/{resource_id}`    | `/v1beta/models`, `/v1beta/models/{model}:generateContent`, `:streamGenerateContent`, `:countTokens`                                 |

- Clients append `/v1/messages` to the Claude base URL and `/v1beta` to the Gemini base URL. Streaming and non-streaming responses are supported. WebSocket response paths are not exposed; an upgrade request is rejected.

### Provider credentials

- Every OAuth and API-key provider that the pinned SDK version supports is allowed. The `Provider` enum of the [protocol](protos-mcpa-v1-contract.md) lists that set: Gemini API keys, Gemini CLI OAuth, Vertex AI, Claude (API key or Claude Code OAuth), Codex (API key or OAuth), Antigravity OAuth, Kimi (OAuth or API key) and OpenAI-compatible upstreams with an API key. A provider that a later SDK version adds is allowed when the SDK pin and the enum are updated together. Service-owned provider accounts are not offered.
- `RegisterApiKeyCredential` stores an API key with a label, the optional `base_url` (required for `OPENAI_COMPATIBLE` and optional for Vertex) and the declared `models` for `OPENAI_COMPATIBLE`. The response returns only non-secret metadata, including `secret_hint`, the last four characters.
- OAuth credentials are connected through a flow that the console or MCP brokers. Managed CPA obtains and refreshes the tokens.
  - `StartProviderOAuth` creates a `ProviderOAuthFlow` of kind `AUTHORIZATION_CODE` or `DEVICE_CODE`, as the provider requires. For `AUTHORIZATION_CODE`, the customer opens `authorization_url`, consents and submits the full final redirect URL from the browser with `CompleteProviderOAuth`. The server validates its state against the flow and exchanges the code. For `DEVICE_CODE`, the customer enters `user_code` at `verification_uri`, and `GetProviderOAuthFlow` polls the provider.
  - A completed flow creates the credential and sets `credential_id`. A failed flow sets a safe `failure_message`. A flow expires at `expire_time`.
  - The provider exchange or poll runs outside the resource lock. Registration then rechecks the caller's `MANAGE` role under that lock. If the caller lost the role meanwhile, the server discards the tokens, fails the flow and returns the authorization error. It creates no credential.
  - PKCE verifiers, device codes and other flow secrets are encrypted at rest and are never returned.
  - Claude, Codex and Kimi flows use the public client IDs of the pinned SDK. Gemini CLI and Antigravity flows use Google installed-app OAuth clients whose IDs and secrets come from operator configuration and never from repository source. Without a configured client, `StartProviderOAuth` for that provider returns `ERROR_REASON_FAILED_PRECONDITION`.
- Token refresh uses a custom SDK token store that writes refreshed tokens back to PostgreSQL encrypted. The SDK file store is never used.
- `RemoveCredential` deletes the credential and unregisters it from the in-process SDK auth state in the same operation. New calls never select it after the response. Calls already sent to the provider continue.
- Other replicas learn about credential changes through PostgreSQL notifications. Registration, removal, refreshed OAuth tokens and resource and tenant deletion queue the notification in the transaction of the change. PostgreSQL delivers it when that transaction commits, so a committed change always notifies, and a change whose notification cannot be queued fails and commits nothing. A replica drops its cached credentials of the resource when the notification arrives, and drops all cached credentials when its listener reconnects.
- No RPC, log, metric, trace, error or audit detail ever contains a provider token or API key.

### Credential encryption and master-key rotation

Credentials use envelope encryption:

- Each secret has its own random 32-byte data key. The secret payload, which is the JSON of the SDK auth metadata including tokens or the API key, is encrypted with AES-256-GCM under the data key.
- The data key is wrapped with AES-256-GCM under the master key named by `MCPA_ACTIVE_KEY_ID`.
- Each row stores `key_id`, `wrapped_key`, `key_nonce`, `nonce` and `ciphertext`. The additional authenticated data binds the credential ID and the resource ID, so a ciphertext cannot be moved to another row.
- `MCPA_MASTER_KEYS` holds every readable master key as a JSON object of key ID to base64url 32-byte key. Startup fails when the active key is missing, a key is malformed or a stored row references a key ID that is not configured.
- OAuth flow secrets and broker and session refresh tokens use the same envelope format.

Master-key rotation replaces the master key without customer-visible downtime:

1. Add the new key to `MCPA_MASTER_KEYS` and set `MCPA_ACTIVE_KEY_ID` to it. Keep the old key.
2. Restart or roll the server. New and refreshed secrets now use the new key, and both keys remain readable.
3. Run `mcpa-api rotate-master-key`. It rewraps each data key under the active master key in bounded batches with row-level concurrency control. It does not re-encrypt payloads and does not block calls or refreshes. It is idempotent and can resume after interruption.
4. Verify that no row references the old key ID. The command reports the number of rows that still reference another key ID.
5. Remove the old key from `MCPA_MASTER_KEYS` and restart.

### Model routing

- **Load balancing.** `LOAD_BALANCING_ROUND_ROBIN` is the only mode. It rotates across the resource's accounts of the selected provider. Selection is strictly scoped to the request's resource and never selects another resource's credential.
- **Allowlist.** An empty `model_allowlist` allows every model that the resource's credentials can serve. A non-empty list allows only exact listed names: the requested model string must equal an entry, so a suffixed name such as `model(high)` is allowed only when that exact string is listed, and listing `model` does not allow it. The server reads the model from the parsed request (the JSON `model` field, or the Gemini path model) and rejects other models with `model_not_allowed` before it sends anything to the provider. A body whose top-level object holds the `model` key more than once (compared after JSON unescaping and without case; nested objects are not inspected) is rejected the same way with a message that does not echo the body, for OpenAI and Claude format routes, whether or not the allowlist is empty, because the body is forwarded unchanged and decoders disagree about which occurrence to read. The recorded status is `CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED` and only the response bytes are billed. The allowlist has at most 200 entries of 1 to 200 bytes each. A requested model name longer than 200 bytes, from the body or from the Gemini path, can never match an entry, so the server rejects it with `model_not_allowed` and the message `the model name is too long` before it reads the allowlist. The message does not echo the name, and the call record and log line carry the name truncated to 200 bytes on a UTF-8 boundary. Names of up to 200 bytes behave as before.
- **No retries.** SDK retries are disabled: the retry count is 0 and the maximum credentials per request is 1. Provider failures, including 429, 5xx and expired or invalid authentication, go back to the caller with the provider's status and body. The server never retries another account or provider.
- **No credential.** When no credential of the resource can serve the model, the call is rejected with `no_credential` and records `CALL_STATUS_NO_CREDENTIAL`.
- Model aliases and fallback ordering are out of scope.

### Call keys

- Managed CPA issues and verifies its own call keys. Each key is bound to one resource, so verification needs no Delcore round trip for the key itself.
- The format is `mcpa_<32 lowercase hexadecimal characters of the key UUID v7 without hyphens>_<43 base64url characters of 32 random bytes>`. The server stores only the SHA-256 hash of the full secret and compares in constant time.
- The full secret is returned once, by the first execution of the `IssueCallKey` request key. A replay returns an empty `secret`. After issuance only `display_prefix` (`mcpa_` plus the first 8 hexadecimal characters and `…`) is shown.
- A key has a name of 1 to 100 bytes, an optional `expire_time` and a `last_used_time`. A new issuance requires a future `expire_time`; a replay returns the original key even after it expired. The server updates `last_used_time` at most once per minute per key.
- The call surface accepts the key from `Authorization: Bearer`, `x-api-key`, `x-goog-api-key` or the Gemini `?key=` query parameter.
- A key is valid only when all of these conditions hold:
  - It is not revoked and not expired.
  - Its resource exists and matches the path resource.
  - The issuer still has `INVOKE` or `MANAGE` on the resource.
  - Delcore `Authorize(invoke_models, paid=true)` for the issuer is allowed. This check fails when the issuer has left the tenant.
- Invokers list and revoke their own keys. Managers list and revoke all keys of the resource. Revocation takes effect for new calls immediately.

### Call path

A request under `/r/{resource_id}/` runs these steps in order. No step before step 7 contacts the provider.

1. Resolve the resource and verify the call key. A failure returns 401 `invalid_call_key`. Requests rejected at this step are not recorded, metered or billed, because they are not attributable to a valid key.
2. Check the issuer's effective role. A missing role returns 401 `invalid_call_key`.
3. Check the tenant state (`ACTIVE`) and `Authorize(invoke_models, paid=true)` through the cache. A `BLOCK_REASON_PERMISSION` denial returns 401 `invalid_call_key` and, like the rejections at steps 1 and 2, is not recorded, metered or billed. A non-`ACTIVE` state or a billing denial returns 403 `tenant_inactive` (`CALL_STATUS_REJECTED_TENANT`). A Delcore error after cache expiry returns 503 `dependency_unavailable` (`CALL_STATUS_REJECTED_DEPENDENCY`).
4. Check the model against the allowlist. A rejection returns 403 `model_not_allowed` (`CALL_STATUS_REJECTED_MODEL_NOT_ALLOWED`).
5. Check the resource and tenant budgets. A rejection returns 429 `budget_exhausted` (`CALL_STATUS_REJECTED_BUDGET`). A request that step 4 rejects never reaches this step, so it neither returns `budget_exhausted` nor increments `mcpa_budget_rejections_total`, whatever the budget state. Calls on resources and tenants without a budget skip the billing period read. Model listings under `/r/{resource_id}/` run the same budget check after the allowlist read.
6. Select a credential by round-robin. When none can serve the model, return 400 `no_credential`.
7. Execute through the embedded SDK and stream or return the provider's response in the requested format.

Rejections use the client format's error shape (OpenAI, Claude or Gemini) with the typed code in `error.type` or the equivalent code field, plus `X-Mcpa-Request-Id`. After a valid key, every call creates a call record, including rejections. A 401 `invalid_call_key` raised after the call state exists, which is a permission denial at step 3 or a resource that is deleted or missing when the allowlist is read at step 4, leaves no call record, outbox row, aggregate change or `mcpa_calls_total` sample. The response still carries `X-Mcpa-Request-Id`, and the request is counted in the HTTP request metric and logged without billing fields. Periodic metering checkpoints skip such a call once the rejection is set, so the 401 response bytes are never billed, even when a checkpoint tick lands while the response is written.

### Byte metering

Billable bytes are the request bytes that Managed CPA writes to the provider plus the response bytes that it writes to the customer.

- **Wire form.** Each segment counts the HTTP/1.1 wire form before TLS: the request line or status line; each header line, `Name: value\r\n`, from the header set at write time; the blank line; and the body bytes as written, after any content encoding. TLS, TCP, chunked transfer framing, HTTP/2 framing and header compression are excluded. The count is the same whatever protocol version the connection negotiates.
- **Counting points.** A counting `http.RoundTripper`, installed through the SDK round-tripper provider, counts the provider segment. A counting `http.ResponseWriter` counts the customer segment.
- **Partial calls.** Bytes already sent are billed whatever the outcome, including provider errors, client cancellation and stream interruption. Rejections after step 1 bill their customer-segment response bytes. Requests sent by the SDK Claude client (Claude credentials and Claude-format Kimi calls) are the exception for request bodies: their body is billed only when the transport reports the whole request written. An upload that is cancelled, rejected or disconnected before it completes bills only its request line and headers.
- **Exclusions.** Provider-reported token counts are never used. Management, console, authentication, OAuth, MCP and operational traffic is never metered.

### Checkpoints and the usage outbox

- **Checkpoints.** Each call registers an in-flight meter. Every `MCPA_CHECKPOINT_INTERVAL`, and at call end, the server commits the unbilled delta of request and response bytes in one transaction. The transaction updates the call record's cumulative bytes and the daily aggregate, and inserts one outbox row when the delta is non-zero.
- **Event identity.** The event key is `mcpa:<call_id>:<checkpoint_seq>`. The event carries `service_id` `MCPA_SERVICE_ID`, event type `mcpa.traffic`, `occurred_at` equal to the call's start time at microsecond precision, and the attributes `bytes` (canonical integer decimal of the delta total) and `resource_id`. Each row keeps one Delcore request key, which every delivery reuses.
- **Crash recovery.** After a crash, only committed checkpoints are billed, exactly once by event key. Bytes after the last checkpoint are not billed. A failed checkpoint is retried at the next interval, increments `mcpa_metering_checkpoint_failures_total` and writes an `alert` error log. Checkpoint failure never blocks the stream.
- **Delivery.** A worker in the same process runs every `MCPA_OUTBOX_INTERVAL`. It deletes expired rows, then selects up to 100 due pending rows, oldest first, and calls `IngestEvent`.
  - An accepted event or an identical replay deletes the row.
  - `ERROR_REASON_INVALID_INPUT`, `ERROR_REASON_INVALID_ATTRIBUTE`, `ERROR_REASON_EVENT_IDENTITY_CONFLICT` and `ERROR_REASON_REPORTING_WINDOW_CLOSED` mark the row `poisoned`. A poisoned row is never sent again, is kept until expiry and raises an alert.
  - Any other result, including transport failures, timeouts, `NO_MATCHING_METER` and `CARDINALITY_EXCEEDED`, keeps the row pending with exponential backoff. The backoff starts at `MCPA_OUTBOX_INTERVAL` and doubles up to one hour.
  - The worker reads a Delcore reason only from a `delcore.v1.ErrorDetail`. It never logs Delcore response bodies.
- **Expiry.** A row expires 30 days after its creation. The expiry is fixed at insertion and is never extended. Before each send, the worker deletes an expired row without sending it and counts it. The charge is then lost and is never estimated.
- **Lateness.** `occurred_at` precedes row creation by up to the call duration. Operators must configure Delcore's `USAGE_LATENESS_SECONDS` to at least 30 days plus the longest expected call duration plus one hour of backoff, and keep `REPORTING_GRACE_SECONDS` above it.
- **Tenant deletion.** Each cycle asks `GetTenantStates` about the tenants of due rows and deletes the rows of tenants that are explicitly `DELETED`.
- **Resource deletion.** Pending rows of deleted resources are still delivered.

### Budgets

- Each resource and each tenant can have an optional USD limit, a canonical non-negative decimal of at most 64 characters. Canonical means digits with an optional fraction: no sign, exponent, leading zeros, bare dot, whitespace or non-numeric value. The server removes trailing fraction zeros (`10.50` becomes `10.5`), rejects longer raw input with `invalid_input`, and stores and returns the limit without rounding. The exhaustion check compares exact decimals; `spent_usd` is the spend rounded to six places. Resource managers set the resource limit. Holders of `manage_tenant_budget` set the tenant limit. An empty value removes the limit.
- The budget period is the tenant's current Delcore billing period, read from `ListMeterAggregates` and cached for `MCPA_AUTHORIZE_CACHE_TTL`.
- Measured spend is the bytes committed locally by checkpoints for calls that started in the period, multiplied by the per-byte price. It uses exact decimal arithmetic.
- Daily usage aggregates supply the UTC days that the period fully covers. Budget spend entries supply the partial first and last days. Each entry holds the committed bytes of the calls of one resource that started at one instant. Spend entries outlive every Delcore billing period, so spend from early in a 31-day period stays counted after its call records expire.
- When spend reaches either limit, new calls are rejected with `budget_exhausted`, and `mcpa_budget_rejections_total{scope}` increments. In-progress streams continue to completion, so measured spend can go slightly over the limit.
- When a limit is set and the period cannot be read after cache expiry, the call fails closed with `dependency_unavailable`.
- A new Delcore billing period resets spend.
- `Budget.exhausted` reports whether spend has reached the limit. Budget warnings and notifications are out of scope.

### Usage, errors and audit

- **Usage.** `GetUsage` returns a `UsageSummary` and daily `UsageRow` aggregates. The summary has bytes per segment, total bytes, total GiB (`total_bytes / 2^30`) and the estimated USD (`total_gib × 0.07`), all as exact canonical decimals without rounding (up to 30 fraction digits for GiB and 32 for USD, trailing zeros removed). Views state that Delcore issues the final bill.
- **Errors.** `ListCallRecords` returns request-level call records. Each record has the call outcome category (`CallStatus`), the provider status class (`StatusClass`), a bounded `error_class` and the debug `request_id`.
- **Filters.** Usage and error views filter by resource, model, provider, call key, status and time range. Without resource filters, tenant-wide results cover the resources on which the caller has the required role.
- **Stored data.** A call record holds only metadata: start time, resource, key, provider, model, format, streaming flag, status, provider status class, error class, request bytes, response bytes, latency and request ID. Request and response bodies (prompts and outputs) are never stored or logged. SDK request-body logging stays disabled, and there is no opt-in debug body logging.
- **Audit.** Each tenant has a customer-visible audit log that resource managers can read through the console and MCP. Events record these actions: resource create, update and delete; credential register and remove; call key issue and revoke; ACL changes; routing changes; and resource and tenant budget changes. Each event records the actor kind, actor user ID, target ID and bounded, secret-free details. Tenant budget events belong to the tenant, not to a resource. `ListAuditEvents` returns a resource's events, and `ListTenantAuditEvents` returns the tenant-level events (tenant budget changes) to holders of `manage_tenant_budget` without granting any resource access.

### Retention

| Record                                     | Retention                                           |
| ------------------------------------------ | --------------------------------------------------- |
| Request-level call records                 | 30 days after the call start                        |
| Budget spend entries                       | 35 days after the call start                        |
| Daily usage aggregates (UTC days)          | 13 months                                           |
| Audit events                               | 13 months                                           |
| Usage outbox rows                          | 30 days after creation                              |
| Request-key idempotency records            | 30 days after first creation                        |
| Provider OAuth flows                       | Deleted one day after completion, failure or expiry |
| Console sessions and revoked broker grants | Deleted after expiry or revocation                  |

A retention worker prunes expired records in bounded batches.

### Deletion and tenant lifecycle

- `DeleteResource` requires `confirm_name` equal to the resource name. In one transaction it does the following:
  - Invalidates and deletes the resource's call keys.
  - Deletes its provider credentials, OAuth flows, routing settings and budget.
  - Unregisters the credentials from the SDK auth state.
  - Leaves a tombstone of the resource ID, tenant, name and deletion time.
- The tombstone and its ACL entries remain read-only. They serve only to show the retained usage, error and audit records to their former roles until retention ends. A deleted resource accepts no other action, and its endpoint rejects calls. Pending billing events continue to be delivered.
- `BLOCKED` and `DELETING` tenants cannot make model calls.
- A lifecycle worker calls `CheckTenantLifecycle` for tenants with retained data. It deletes all of a tenant's retained data (resources, tombstones, ACL, credentials, keys, records, budget spend entries, aggregates, audit, snapshots and outbox rows) only after Delcore reports `DELETED`. `UNKNOWN`, errors and pending deletion delete nothing. Fixed record expiry applies independently.

### Error mapping

Every Connect error carries `mcpa.v1.ErrorDetail` with its reason and trace ID. This includes a request body that fails to decode as JSON or binary protobuf, such as malformed JSON or a wrongly typed field: the management route answers `invalid_argument` with `ERROR_REASON_INVALID_INPUT`, the fixed message `request body is not a valid message` and no request content, and logs one `management call rejected` line. A body that fails to decode is rejected before authentication.

| Reason                                     | Connect code          |
| ------------------------------------------ | --------------------- |
| `ERROR_REASON_INVALID_INPUT`               | `invalid_argument`    |
| `ERROR_REASON_UNAUTHENTICATED`             | `unauthenticated`     |
| `ERROR_REASON_PERMISSION_DENIED`           | `permission_denied`   |
| `ERROR_REASON_NOT_FOUND`                   | `not_found`           |
| `ERROR_REASON_ALREADY_EXISTS`              | `already_exists`      |
| `ERROR_REASON_FAILED_PRECONDITION`         | `failed_precondition` |
| `ERROR_REASON_MFA_REQUIRED`                | `permission_denied`   |
| `ERROR_REASON_DEPENDENCY_UNAVAILABLE`      | `unavailable`         |
| `ERROR_REASON_TENANT_INACTIVE`             | `failed_precondition` |
| `ERROR_REASON_BUDGET_EXHAUSTED`            | `resource_exhausted`  |
| `ERROR_REASON_MODEL_NOT_ALLOWED`           | `permission_denied`   |
| `ERROR_REASON_IDEMPOTENCY_CONFLICT`        | `failed_precondition` |
| `ERROR_REASON_TENANT_SERVICE_KEY_REJECTED` | `permission_denied`   |

Error messages never echo secrets or request content.

## Storage

PostgreSQL is authoritative. All entity IDs are UUID v7. The schema holds these tables:

- Resources and tombstones.
- ACL entries.
- Encrypted credentials.
- Encrypted provider OAuth flows.
- Call keys (hash, prefix, issuer, expiry, last use and revocation).
- Request-key idempotency records.
- Call records.
- Budget spend entries (tenant, resource, call start instant and committed bytes).
- Daily usage aggregates.
- Resource and tenant budgets.
- Audit events.
- The usage outbox.
- Group-membership snapshots.
- Console sessions and MCP broker grants, with their encrypted Logto tokens.
- The schema version table.

Nothing stores request or response bodies.

Migrations are explicit, ordered and immutable SQL files embedded in the binary. `mcpa-api migrate` applies them in one transaction under an advisory lock and records each version and digest; repeating it is a no-op. `serve` and `/readyz` refuse a schema that differs from the binary's expected version or digests. `check-schema` performs the same check without serving. Normal startup never migrates. Recovery is fix-forward only under the [service migration and recovery policy](repository-workflow-contract.md#service-migration-and-recovery): correct defects with a new version and a new forward migration. Never roll back images or apply reverse migrations.

The data plane uses no object storage, so the Cloudflare R2 default does not apply.

## Security

- **Secrets.** Provider tokens and API keys, call key secrets, Delcore personal keys, Logto tokens, master keys, the machine-to-machine client secret, the Logto web client secret, the metrics token and the database URL never appear in logs, metrics, traces, errors, audit details or responses. Configuration types print them redacted. Call key secrets are stored only as hashes.
- **Isolation.** Every lookup is scoped by tenant and resource from the verified credential, never from an unverified request field. A call key works only for its own resource. Cross-tenant and cross-resource use is rejected. Credential selection never crosses resources.
- **Audiences.** Console tokens (`MCPA_API_RESOURCE`), MCP tokens (`MCPA_MCP_RESOURCE`) and Delcore tokens (`DELCORE_RESOURCE`) are not interchangeable. MCP and console tokens are never forwarded to Delcore. The server verifies each token's issuer through `LOGTO_ISSUER` discovery and JWKS. Discovery is lazy and shared: concurrent callers during a Logto outage share one discovery attempt bounded by `DEPENDENCY_TIMEOUT` and fail together as dependency unavailable, a caller that cancels stops waiting without aborting the attempt for the others, and only a successful discovery is cached. The server checks the signature, expiry and audience. One signing-key cache serves every verification and is refetched only when a token names an unknown key ID, so a token that fails signature, issuer, audience, expiry or nonce checks never causes a Logto request when its key is cached. A failure to obtain signing keys (network error, timeout, non-200 status or an unusable key set) is a Logto outage, not a token failure: management and `/mcp` authentication return `ERROR_REASON_DEPENDENCY_UNAVAILABLE` (`/mcp` answers 503 `dependency_unavailable` without a `WWW-Authenticate` challenge), the MCP authorization callback redirects with `temporarily_unavailable`, and the next verification fetches again. Token failures remain `ERROR_REASON_UNAUTHENTICATED` (`/mcp` answers 401 `invalid_token`).
- **Browser surfaces.** Cookie sessions require CSRF and `Origin` checks. Redirects accept only exact registered URLs. The call surface and `/mcp` reject credentialed cross-origin access.
- **URLs.** `DELCORE_URL`, `LOGTO_TOKEN_URL`, `LOGTO_ISSUER` and the public URLs must be `https`, or `http` on loopback.
- **Provider destinations.** A customer `base_url` must be an `https` URL whose host is a DNS name or a public IP literal. This check is only the first line of defense. After DNS resolution, the data plane refuses to dial private, loopback, link-local, shared-address and unique-local addresses, and it ignores proxy environment variables. The guard applies to every provider client. The pinned SDK's Claude client ignores the shared provider transport, so the data plane sends that client's connections through an in-process loopback SOCKS5 relay that resolves and dials with the same guard. The relay requires a random secret that exists only in the server process, so other local processes cannot use it as an egress proxy. A refused destination fails the call; no provider request is sent.
- **Provider disclosure.** Request content is forwarded to the customer's own provider account, under that provider's terms. Managed CPA keeps no copy.
- **Operational endpoints.** `/healthz` and `/readyz` disclose no configuration. Deployments restrict `/metrics` with `MCPA_METRICS_TOKEN` or network policy.
- **SQL.** Use parameterized SQL only.

## Logging

- **Logs.** Use `log/slog` structured logs. The default text handler uses ANSI color. Setting `NO_COLOR` to any value, or setting `LOG_FORMAT=json`, disables color; `json` emits JSON lines. Logs carry the request ID, trace ID, surface, procedure or route, tenant and resource IDs where relevant, bounded outcome categories and durations. The logger adds `trace_id` (equal to `X-Mcpa-Request-Id`) and `surface` to every line written with a request context on the Connect, `/auth`, `/oauth` and `/mcp` surfaces, so call sites must not add those keys. Lines on the call surface carry `surface` and their call record ID instead of `trace_id`, because that surface replaces the request ID with the call record ID. Startup and background lines carry neither key. They never contain secrets or bodies. Checkpoint failures, poisoned or expired outbox rows and master-key problems log at error level with `alert=true`.
- **Metrics.** Prometheus metrics use bounded labels only. They have no tenant, resource, key, user or model labels and no secrets.

| Metric                                    | Type      | Labels                                                                  |
| ----------------------------------------- | --------- | ----------------------------------------------------------------------- |
| `mcpa_calls_total`                        | counter   | `format`, `provider`, `status` (`CallStatus` short name)                |
| `mcpa_call_duration_seconds`              | histogram | `format`, `status`                                                      |
| `mcpa_provider_failures_total`            | counter   | `provider`, `status_class` (`429`, `4xx`, `5xx`, `auth`, `network`)     |
| `mcpa_delcore_failures_total`             | counter   | `rpc`, `kind` (`unavailable`, `denied`, `error`)                        |
| `mcpa_outbox_pending`                     | gauge     | none                                                                    |
| `mcpa_outbox_poisoned`                    | gauge     | none                                                                    |
| `mcpa_outbox_oldest_pending_age_seconds`  | gauge     | none                                                                    |
| `mcpa_outbox_delivery_failures_total`     | counter   | `category`                                                              |
| `mcpa_metering_checkpoint_failures_total` | counter   | none                                                                    |
| `mcpa_budget_rejections_total`            | counter   | `scope` (`resource`, `tenant`)                                          |
| `mcpa_billable_bytes_total`               | counter   | `segment` (`request`, `response`)                                       |
| `mcpa_http_requests_total`                | counter   | `surface` (`management`, `proxy`, `mcp`, `auth`, `oauth`), `code_class` |

- **Traces.** OpenTelemetry traces are exported through OTLP when `OTEL_EXPORTER_OTLP_ENDPOINT` is set and are disabled otherwise. They use the standard `OTEL_*` variables. Spans carry no secrets, bodies or high-cardinality customer identifiers beyond the request ID.
- **Database call bound.** In `serve`, every request-path database call (call authentication and gates, management and session reads and writes, MCP and console sign-in state) ends within `DEPENDENCY_TIMEOUT`. The store opens the pool with the PostgreSQL `statement_timeout` and `lock_timeout` set to that duration (rounded up to whole milliseconds), so a blocked lock or a slow statement ends on the server even inside a transaction, which bounds each statement rather than the whole transaction. Calls outside a transaction and every wait for a pooled connection also carry a context deadline of the same length, so a saturated pool fails instead of queueing until the request ends. These failures are `ERROR_REASON_DEPENDENCY_UNAVAILABLE` for management (including PostgreSQL SQLSTATE `57014` and `55P03`), the MCP and OAuth equivalents, and a `dependency_unavailable` rejection (`CALL_STATUS_REJECTED_DEPENDENCY`) on the call surface. `mcpa-api migrate`, `check-schema` and `rotate-master-key` run without these bounds so long statements keep working. Tenant data deletion keeps its own bound of four times `DEPENDENCY_TIMEOUT` (transaction-local `statement_timeout` and `lock_timeout`), and retention batches keep their per-batch `DEPENDENCY_TIMEOUT`. The connection used for credential-change `LISTEN` is bounded only while it waits for a pooled connection. A connection pooler that rejects startup parameters must be configured to accept `statement_timeout` and `lock_timeout`.
- **Health.** `/healthz` reports that the process is alive. `/readyz` reports ready only when the database is reachable and the schema is current.
- **Operator assets.** `servers/mcpa-api/ops/` follows the layout of `servers/lane-api/ops/`. It holds Prometheus alert and recording rules, a dashboard, rule tests and a README runbook. Alerts cover call errors, provider failures, Delcore authorization failures, outbox backlog and age, poisoned rows, metering checkpoint failures and budget rejections. Thresholds come from operator configuration and are operational triggers, not SLOs. The assets contain no secrets or request bodies.

## Build and Test

The package scripts of `servers/mcpa-api/package.json` are the fixed interface for CI and Turbo. They run from the repository root through `scripts/ci/from-root.mjs`:

| Script           | Command                                                                                                 |
| ---------------- | ------------------------------------------------------------------------------------------------------- |
| `dev`            | Infisical `/mcpa-api` injection, then Portless `mcpa-api` on port `46329`, then the shared API launcher |
| `build`          | `go build -o servers/target/debug/mcpa-api ./servers/mcpa-api`                                          |
| `ci:format`      | Go formatting check for `servers/mcpa-api`                                                              |
| `ci:vet`         | `go vet ./servers/mcpa-api/...`                                                                         |
| `ci:unit`        | `go test ./servers/mcpa-api/...`                                                                        |
| `ci:integration` | `go test -tags integration ./servers/mcpa-api/...`                                                      |
| `ci:vuln`        | `govulncheck` at a pinned version for `./servers/mcpa-api/...`                                          |
| `ci:oci`         | `docker build -f servers/mcpa-api/Dockerfile .` for local packaging checks                              |

Unit tests run without external services. Integration tests use the build tag `integration` and a disposable PostgreSQL from `TEST_DATABASE_URL`. When it is unset, they skip with a clear message. They mock Logto, Delcore and providers. Root `go test ./...` and `go vet ./...` also cover the package.

Tests cover these scenarios:

- **Happy path.** Create a resource, connect a mocked OAuth credential and an API key, and issue a key. Make streaming and non-streaming calls in each of the three formats. Check usage and the estimated USD, and check that a mocked Delcore receives a matching `IngestEvent`.
- **Byte metering.** Compressed and uncompressed bodies, chunked streams, client cancellation in mid-stream, interrupted uploads, provider disconnects and provider 4xx and 5xx responses. Management traffic is not metered.
- **Crash recovery.** Kill the process during a stream. After restart, bytes up to the last checkpoint are billed exactly once, with no duplicate events.
- **Outbox.** Delcore is unavailable for less than 30 days, then events are delivered once. Events older than 30 days are dropped without estimating. Poisoned rows are covered.
- **Authorization.**
  - `manage` and `invoke` roles combined with Delcore capabilities.
  - Group membership changes through the snapshot.
  - Issuer role loss and tenant departure invalidating keys.
  - Cross-tenant and cross-resource key use rejected.
  - Tenant service keys rejected.
  - Personal keys limited to their owner's ACL.
  - The last-manager rule.
- **MFA.** Console and MCP tokens without evidence are rejected with the reauthentication URL, and refresh without evidence fails.
- **MCP.** Metadata discovery, PKCE login through the broker, exact redirect validation, the audience never forwarded to Delcore and every RPC available as a tool.
- **Billing gates.** `paid=true` denial; `BLOCKED`, `DELETING` and `UNKNOWN` tenants; cache TTL expiry while Delcore is down; denials never cached.
- **Routing.** Round-robin distribution, empty and non-empty allowlists, and no retry after provider failure.
- **Budgets.** Resource and tenant limits each block new calls. An in-progress stream finishes after the limit. Spend resets at the Delcore billing period boundary.
- **Credentials.** Encryption at rest with a key ID, AAD binding, master-key rotation with concurrent calls and refreshes, immediate removal, and removal propagation to other replicas through notifications that commit with the change. No secrets appear in logs, metrics or traces.
- **Retention and deletion.** 30-day and 13-month pruning, resource deletion and tombstone visibility, and tenant cleanup only after `DELETED`.

CI runs format, vet, unit tests, integration tests against a disposable PostgreSQL and `govulncheck` for affected changes on Ubuntu. Release image publication builds the OCI image. See the [workflow contract](repository-workflow-contract.md#managed-cpa-development-and-validation).

## Dependencies and Integrations

- **CLIProxyAPI SDK.** `github.com/router-for-me/CLIProxyAPI/v6` is pinned in the root `go.mod` and licensed under MIT. Ship its license text and copyright notice in the third-party section of `servers/NOTICE`, and include `servers/LICENSE` and `servers/NOTICE` in the image. Upgrading the SDK updates the `Provider` enum, the endpoint routes and this contract when they change. CLIProxyAPI removed built-in usage statistics in v6.10.0, so Managed CPA meters bytes itself.
- **Dependency security.** Run `govulncheck` under the [dependency security contract](repository-dependency-security-contract.md) and record results and unresolved findings in CI or PR records.
- **Delcore.** Follow the [Delcore project](project-delcore.md), [server](servers-delcore-api-contract.md) and [protocol](protos-delcore-v1-contract.md) contracts. They cover consumer obligations, 30-day usage retention and tenant states.
- **Logto.** Logto supplies identity, the web application, the API resources `MCPA_API_RESOURCE` and `MCPA_MCP_RESOURCE`, the machine-to-machine application and the custom access-token claims script for MFA evidence.
- **Image.** The OCI image is `ghcr.io/<owner>/mcpa-api:sha-<commit>` with FSL-1.1-ALv2 labels, built from the root checkout with `servers/mcpa-api/Dockerfile`. It never includes environment files or credentials.

## Change Triggers

Update the [project index](project-mcpa.md), the [protocol contract](protos-mcpa-v1-contract.md) and the [web console contract](apps-mcpa-web-contract.md) together with this contract when interfaces, authorization, billing, retention or surfaces change. Update the [environment contract](repository-environment-contract.md) for configuration ownership changes and the [workflow contract](repository-workflow-contract.md) for CI, development or image changes. Update `AGENTS.md` only for ownership or development-procedure changes under the [instruction-update policy](README.md#instruction-update-policy).

## Environment configuration

Shared settings use unprefixed names; Managed CPA settings use the `MCPA_` prefix. A required value that is missing or invalid stops startup with an error that names the setting and omits its value. `servers/mcpa-api/.env.example` contains names, placeholders and safe loopback defaults only.

| Name                                                                       | Meaning                                                                                                                   |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `LISTEN`                                                                   | Listen address. The default is `127.0.0.1:46329`.                                                                         |
| `DATABASE_URL`                                                             | PostgreSQL URL. Secret.                                                                                                   |
| `MCPA_PUBLIC_URL`                                                          | Public API origin, for example `https://mcpa.api.delino.io`. Endpoint base URLs derive from it.                           |
| `WEB_ORIGIN`                                                               | Console origin, for example `https://mcpa.delino.io`, used for CORS and `Origin` checks.                                  |
| `MCPA_WEB_RETURN_URLS`                                                     | JSON array of the exact allowed console `return_to` URLs.                                                                 |
| `MCPA_MASTER_KEYS`                                                         | JSON object of key ID to base64url 32-byte master key. Secret.                                                            |
| `MCPA_ACTIVE_KEY_ID`                                                       | Key ID used for new encryptions.                                                                                          |
| `MCPA_SERVICE_ID`                                                          | Delcore service UUID v7 of `mcpa`, used as the event `service_id`.                                                        |
| `DELCORE_URL`, `DELCORE_RESOURCE`                                          | Delcore Connect base URL and Logto resource indicator.                                                                    |
| `LOGTO_TOKEN_URL`, `CLIENT_ID`, `CLIENT_SECRET`                            | Machine-to-machine credentials for Delcore (client credentials). `CLIENT_SECRET` is secret.                               |
| `LOGTO_ISSUER`                                                             | Logto OIDC issuer for discovery and JWKS.                                                                                 |
| `MCPA_LOGTO_CLIENT_ID`, `MCPA_LOGTO_CLIENT_SECRET`                         | Logto web application used by the console BFF and the MCP broker. The secret is secret.                                   |
| `MCPA_GEMINI_CLI_OAUTH_CLIENT_ID`, `MCPA_GEMINI_CLI_OAUTH_CLIENT_SECRET`   | Optional Google installed-app OAuth client for Gemini CLI account connection. Set both or neither. The secret is secret.  |
| `MCPA_ANTIGRAVITY_OAUTH_CLIENT_ID`, `MCPA_ANTIGRAVITY_OAUTH_CLIENT_SECRET` | Optional Google installed-app OAuth client for Antigravity account connection. Set both or neither. The secret is secret. |
| `MCPA_API_RESOURCE`                                                        | Logto API resource (audience) for console sessions, for example `https://mcpa.api.delino.io`.                             |
| `MCPA_MCP_RESOURCE`                                                        | MCP-only audience, for example `https://mcpa.api.delino.io/mcp`.                                                          |
| `MCPA_MFA_CLAIM`                                                           | Boolean access-token claim that carries MFA evidence. The default is `mcpa_mfa`.                                          |
| `MCPA_MFA_REAUTH_URL`                                                      | URL where users enroll in MFA or reauthenticate.                                                                          |
| `MCPA_OAUTH_CLIENTS`                                                       | JSON registry of approved MCP clients: `{"<client_id>":{"name":"...","redirect_uris":["..."]}}`.                          |
| `MCPA_AUTHORIZE_CACHE_TTL`                                                 | Go duration for caching allowed Delcore results, for example `30s`. Required.                                             |
| `MCPA_CHECKPOINT_INTERVAL`                                                 | Go duration of the metering checkpoint interval, for example `5s`. Required.                                              |
| `MCPA_OUTBOX_INTERVAL`                                                     | Go duration of the outbox delivery poll interval.                                                                         |
| `MCPA_METRICS_TOKEN`                                                       | Optional bearer token for `/metrics`. Secret.                                                                             |
| `DEPENDENCY_TIMEOUT`                                                       | Go duration for Delcore and database calls; see the database call bound in Observability.                                 |
| `LOG_FORMAT`                                                               | `text` (default, ANSI color) or `json`.                                                                                   |
| `NO_COLOR`                                                                 | Any value disables ANSI color in text logs.                                                                               |
| `OTEL_*`                                                                   | Standard OpenTelemetry exporter variables. Tracing is disabled when `OTEL_EXPORTER_OTLP_ENDPOINT` is unset.               |

Secrets live in the operator's deployment target or the `/mcpa-api` Infisical development folder. They never live in Git, workflow files, frontend environments or logs. See the [environment contract](repository-environment-contract.md#managed-cpa-development-environment).

## Local development

- Run root `pnpm dev`, or service-local `pnpm dev` in `servers/mcpa-api`. The `dev` script injects the `/mcpa-api` Infisical folder, registers the Portless route `mcpa-api` and runs the shared `servers/scripts/dev-api.mjs` launcher. The launcher fixes `LISTEN` to `127.0.0.1:46329`, applies the Compose database URL after injection and executes `servers/target/debug/mcpa-api serve --dev-env`. The development build writes that binary with `go build`; the launcher does not build or watch.
- Root Compose provides `mcpa-postgres` on `127.0.0.1:${MCPA_POSTGRES_PORT:-55438}` with database and user `mcpa` and the disposable password `local-mcpa`: `postgres://mcpa:local-mcpa@127.0.0.1:55438/mcpa?sslmode=disable`. These values are for local development only.
- Run `mcpa-api migrate` explicitly before the first start and after schema changes. Startup checks the schema and refuses a mismatch.
- The console runs on `127.0.0.1:46328` (Portless `mcpa`). Set `WEB_ORIGIN` and `MCPA_WEB_RETURN_URLS` to the printed console origin. Register the callback URLs with the development Logto applications.
- Load the catalog example under `servers/mcpa-api/catalog/` into the development Delcore so that the service, meter and price exist.

## Deployment

This repository publishes the OCI image only. Runtime rollout, migrations in deployed environments, infrastructure, deployment-specific monitoring settings and Delcore catalog registration belong to private operator automation and operators. Operators apply migrations explicitly and verify the deployed revision and readiness before exposure. Recovery is fix-forward only.

## Known limits

- **Group snapshot.** Group ACL entries resolve through a per-tenant snapshot of Delcore group membership that console and MCP sessions refresh. A group membership change in Delcore takes effect for call keys and personal-key management only after the next snapshot refresh in that tenant. Until then, a user removed from a group can keep a group-granted role. Direct user entries, revocation, Delcore capabilities and tenant membership are checked live through `Authorize`. Leaving the tenant therefore invalidates keys within the authorization cache TTL. Remove this limit when Delcore offers registered services a machine-to-machine group-membership read.
- **Crash window.** Bytes sent after the last committed checkpoint before a crash are not billed.
- **Budget overshoot.** In-progress streams and authorization lag can exceed a budget slightly.
- **Rotation and restarts.** Master-key rotation requires that every server instance has the new key before rewrapping starts.

## References

- [Project index](project-mcpa.md)
- [Protocol contract](protos-mcpa-v1-contract.md)
- [Web console contract](apps-mcpa-web-contract.md)
- [Repository defaults](repository-defaults.md)
- [Repository boundary](repository-boundary-contract.md)
- [License contract](repository-license-contract.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)
- [Lane API contract](servers-lane-api-contract.md)
- [DeliTools API contract](servers-delitools-api-contract.md)
