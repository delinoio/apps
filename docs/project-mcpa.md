# Project: Managed CPA

## Goal

Provide Managed CPA, a multi-tenant service for external customers in Delcore personal and organization tenants. Customers call models through their own provider OAuth accounts or API keys. A Go server embeds the [CLIProxyAPI SDK](https://github.com/router-for-me/CLIProxyAPI/blob/main/docs/sdk-usage.md) and gives each customer resource its own path-based model-call endpoint at `https://mcpa.api.delino.io`. Customers manage resources through the console at `https://mcpa.delino.io` and through an HTTP MCP endpoint.

Customers pay their providers directly. Delcore bills Managed CPA usage separately: USD 0.07 per GiB (2^30 bytes) of measured traffic, with no free allowance and no per-request fee.

The project is complete when an external tenant can do all of the following without operator help:

1. Create a resource.
2. Connect a provider credential.
3. Issue a call key.
4. Call a model.
5. View usage and errors.
6. Receive the matching Delcore usage charge.

There is no deadline and no numeric success metric. This index and its domain contracts define required behavior; they do not establish that the service is implemented or deployed.

## Project ID

`mcpa` (display name Managed CPA). The console workspace is `@apps/mcpa` and the API workspace is `@servers/mcpa-api`. The Delcore service key, the Portless console route and the protocol package prefix also use `mcpa`.

## Domain Ownership Map

- `apps/mcpa`: React and Rsbuild web console served at `https://mcpa.delino.io`. Apache-2.0. Owned by the [web console contract](apps-mcpa-web-contract.md).
- `servers/mcpa-api`: Go API server served at `https://mcpa.api.delino.io`. It owns the Connect management service, console authentication, Streamable HTTP MCP and its OAuth broker, the embedded CLIProxyAPI data plane, byte metering, the Delcore usage outbox, budgets, retention and tenant-lifecycle workers, explicit SQL migrations, OCI packaging, the Delcore catalog example under `servers/mcpa-api/catalog/` and reusable operator assets under `servers/mcpa-api/ops/`. FSL-1.1-ALv2. Owned by the [API server contract](servers-mcpa-api-contract.md).
- `protos/mcpa/v1`: `mcpa.v1` Connect schema with checked-in generated Go (`mcpav1`, `mcpav1connect`) and TypeScript (`@repo/protos/mcpa/v1/mcpa_pb`) bindings. Apache-2.0. Owned by the [protocol contract](protos-mcpa-v1-contract.md).
- `servers/LICENSE` and `servers/NOTICE`: the server distribution license and notices, including the CLIProxyAPI MIT license text and copyright notice.
- `.github/workflows/deploy-images.yml`: publishes `ghcr.io/<owner>/mcpa-api:sha-<commit>`; see the [workflow contract](repository-workflow-contract.md#managed-cpa-development-and-validation).

Server runtime rollout, infrastructure and deployment-specific monitoring settings belong to private operator automation. This checkout owns the runtime source, migrations, safe configuration examples, the OCI image and reusable monitoring assets with their validation.

## Domain Contract Documents

- [API server](servers-mcpa-api-contract.md)
- [Connect protocol](protos-mcpa-v1-contract.md)
- [Web console](apps-mcpa-web-contract.md)

## Cross-Domain Invariants

### Language deviation

`servers/mcpa-api` uses Go, not the repository Rust default for new servers. The server embeds the Go CLIProxyAPI SDK (`github.com/router-for-me/CLIProxyAPI/v6/sdk/cliproxy`) in-process, and that SDK has no Rust equivalent. The server is a package tree inside the root `github.com/delinoio/apps` Go module, so it consumes `protos/mcpa/v1` and `protos/delcore/v1` directly. It adds no `go.work`, no local `replace` directive and no separate Go module. New Go dependencies go in the root `go.mod` and `go.sum`. It is not a member of the server Cargo workspace. The [repository boundary contract](repository-boundary-contract.md) and the [server Rust workspace contract](servers-rust-workspace-contract.md) record this exception.

### Execution and resource model

- One Go process embeds the SDK. There is no CPA server or process per resource and no action to create, start or stop a dedicated CPA server.
- A tenant can create and delete multiple resources. Each resource has independent provider credentials, routing settings, call keys, ACL, budget and endpoint. Every resource belongs to exactly one Delcore tenant; the tenant is the namespace.
- Persisted entity IDs, including resource, credential, call key, call record, OAuth flow, audit event and outbox IDs, are UUID v7.
- Endpoints are path-based: `https://mcpa.api.delino.io/r/{resource_id}/...`. The SDK's OpenAI-compatible routes (including Responses), Claude-compatible routes and Gemini-compatible routes are mounted under that prefix, with streaming and non-streaming responses. WebSocket response paths are not exposed. The console and MCP show the base URL of each supported format.

### Identity, authorization and billing

- Logto owns identity. Delcore owns tenants, memberships, groups, service capabilities and USD usage billing. Delcore stays service-neutral: it knows Managed CPA only as the registered service `mcpa` with capabilities, a meter and a price. Do not add AI-, model- or mcpa-specific types or branches to `servers/delcore-api` or `protos/delcore/v1`.
- Effective access requires both a Delcore capability and a per-resource role (`manage` or `invoke`) that Managed CPA stores. Delcore has no resource-level permissions and no MFA rules; Managed CPA owns both.
- Console and interactive MCP access require Logto login with MFA evidence in the token. Non-interactive management uses Delcore personal service keys (`dlk_`), which act with their owner's per-resource ACL and are exempt from MFA. Tenant service keys are rejected for management.
- Model calls require Delcore `Authorize` with `paid=true` and an `ACTIVE` tenant. Management operations use `paid=false`. Paid authorization fails closed when Delcore cannot be reached after the authorization cache TTL expires.
- Billable bytes are the request bytes Managed CPA sends to the provider plus the response bytes it sends to the customer, counted as HTTP headers and body before TLS. Management, console, authentication and MCP traffic is free and never metered. Provider-reported token counts are never used.
- The Delcore meter is `mcpa_traffic_bytes` (event type `mcpa.traffic`, sum of attribute `bytes`), priced by price key `mcpa_traffic` at USD 0.07 per GiB with zero included usage and no per-request fee. Delcore issues the final bill; Managed CPA usage views show an estimate.
- Pending usage reports follow the shared Delcore consumer policy: they are retried for 30 days after first creation and then deleted without estimating.
- Only an explicit Delcore `DELETED` tenant state permits automatic deletion of retained tenant data.

### Data handling

- Customers own their provider credentials and costs. Service-owned provider accounts are not offered.
- Provider tokens and API keys are stored in PostgreSQL with application-level envelope encryption under an operator master key. Each ciphertext records its key ID. Secrets are never returned after creation and never logged.
- Managed CPA stores call metadata only. It never stores or logs request or response bodies (prompts and outputs), including through opt-in debug logging. SDK request-body logging stays disabled.
- Request-level call records are retained for 30 days. Daily aggregates and the customer-visible audit log are retained for 13 months.

### Protocol and surfaces

- Connect over `mcpa.v1.McpaService` is the management interface. The console, interactive MCP sessions and Delcore personal keys use the same handlers and the same authorization rules. MCP exposes every management feature as a tool.
- The console consumes the protocol through `@connectrpc/connect-query` and the private `@repo/protos` workspace. It holds no service credentials and no provider secrets beyond the input field that submits an API key once.

### Repository and licensing

- `servers/mcpa-api` files use FSL-1.1-ALv2. The server distribution includes `servers/LICENSE` and `servers/NOTICE`, and `servers/NOTICE` carries the CLIProxyAPI MIT license text and copyright notice. The protocol and console use Apache-2.0.
- The console is a static Rsbuild site deployed to Cloudflare Pages under the repository default. It is English only.
- Development uses fixed loopback ports `46328` (console, Portless `mcpa`) and `46329` (API, Portless `mcpa-api`), the root Compose database `mcpa-postgres` on `${MCPA_POSTGRES_PORT:-55438}` and the Infisical development folder `/mcpa-api`.
- Migrations are explicit; the server checks the schema at startup and never migrates while serving. Recovery is fix-forward only.
- Deployment from this repository is image publication only.
- Public documentation in `apps/delino-docs` is out of scope.
- There is no feature flag. Deployment and Delcore catalog registration control exposure.

### Out of scope

- Dedicated CPA servers per resource, and actions to create, start or stop them.
- Service-owned provider accounts.
- WebSocket response paths.
- Model aliases and fallback ordering, and retries after provider failures.
- Storing or logging request or response bodies, including opt-in debug body logging.
- Budget warning or limit notifications (email or other channels).
- Fixed operator rate or concurrency limits.
- Numeric SLO, latency or capacity targets.
- Public documentation in `apps/delino-docs`.
- Console localization other than English.
- Feature flags.
- Tenant service keys for Managed CPA management.
- Runtime deployment automation, which private operator automation owns.

## Change Policy

Update this index, the affected server, protocol and console contracts and the [documentation catalog](README.md) together when paths, interfaces, authorization rules, billing rules, retention or deployment boundaries change. Protocol changes preserve field numbers and enum values and update the server and console consumers in the same change. Changes to the Delcore service registration, meter or price update the server contract and the catalog example together; they never add Managed CPA logic to Delcore.

Record component behavior changes for `@apps/mcpa` and `@servers/mcpa-api` under the [changeset contract](repository-changeset-contract.md), including records for the new components when they are introduced.

Update `AGENTS.md` files only when development procedures, directory ownership or repository/domain development rules change; follow the [instruction-update policy](README.md#instruction-update-policy). Record implementation status, validation results and unresolved limits in pull requests, issues and CI artifacts. Do not treat these contracts as evidence of implementation.

## References

- [Repository defaults](repository-defaults.md)
- [Repository boundary](repository-boundary-contract.md)
- [License contract](repository-license-contract.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)
- [Dependency security contract](repository-dependency-security-contract.md)
- [Changeset contract](repository-changeset-contract.md)
- [Delcore project](project-delcore.md)
- [Lane API contract](servers-lane-api-contract.md) (MCP, OAuth and browser-session pattern)
- [DeliTools API contract](servers-delitools-api-contract.md) (Delcore usage outbox pattern)
