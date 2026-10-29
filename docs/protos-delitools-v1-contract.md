# DeliTools v1 protocol contract

## Scope

This contract belongs to `delinoio/apps`. The schema and generated Go and TypeScript messages live together in `protos/delitools/v1`. Connect-Go bindings live in its `delitoolsv1connect` subdirectory. The server behavior is owned by the [server contract](servers-delitools-api-contract.md).

## Runtime and Language

Protobuf package `delitools.v1` with service `DelitoolsService`. Generated Go clients and handlers use the root `github.com/delinoio/apps` module. TypeScript bindings are exported by the private `@repo/protos` workspace as `@repo/protos/delitools/v1/delitools_pb`. Rust bindings are generated into Cargo output by `servers/crates/delino-protos`; do not check them in. There is no REST surface, handwritten SDK or published client package.

## Users and Operators

External developers and their servers using Delino login tokens, personal API keys or tenant service API keys; operators.

## Interfaces and Contracts

`DelitoolsService` exposes two Connect unary RPCs, `Translate` and `Extract`. Both authenticate with `Authorization: Bearer <credential>`; the server contract owns authentication, authorization, limits and billing.

- `TranslateRequest`: `tenant_id` (required for Logto user tokens; empty or equal to the key's tenant for API keys), `texts` (non-empty strings), `target_language` (required BCP 47 tag), `source_language` (optional BCP 47 tag), `model` (operator-allowed Gateway model identifier). `TranslateResponse` returns `request_id` and `translations` with the order and length of `texts`. Any item failure fails the request.
- `ExtractRequest`: `tenant_id`, non-empty `text` and `json_schema`, a JSON Schema document in the supported subset documented by the server contract. The model is fixed by the server and is not a request field. `ExtractResponse` returns `request_id` and `json`, a JSON document that conforms to the accepted schema.
- `ErrorDetail` is attached to every error that a handler produces and carries `reason` and `request_id`. A JSON request body that cannot be decoded, such as a field of the wrong type, truncated JSON or a syntax error, is rejected before any handler runs with `invalid_argument`, `ErrorDetail` reason `INVALID_INPUT`, a UUID v7 `request_id` that matches `X-Delitools-Request-Id`, and the fixed message `request body is invalid`; the message never contains decoder diagnostics or submitted content. One transport-level exception exists: a message above the transport bound is rejected before any handler runs with a plain Connect `resource_exhausted` error that has no `ErrorDetail` and no request ID. The stable `ErrorReason` values are `INVALID_INPUT`, `UNAUTHENTICATED`, `PERMISSION_DENIED`, `BILLING_BLOCKED`, `LIMIT_EXCEEDED`, `RATE_LIMITED`, `MODEL_UNAVAILABLE`, `UNSUPPORTED_SCHEMA`, `PROVIDER_FAILED`, `INVALID_OUTPUT`, `EXTRACTION_FAILED`, `TIMEOUT`, `CANCELED` and `DEPENDENCY_UNAVAILABLE`. The server contract maps each to its Connect code and defines its meaning. Messages are English. An unexpected internal failure carries the Connect `internal` code with the unspecified reason. Responses do not carry usage or charges; delcore reports them.

Preserve field numbers and enum values. `ERROR_REASON_MODEL_UNAVAILABLE` and `ERROR_REASON_PROVIDER_FAILED` each map to two Connect codes, as the server contract lists. New reasons and fields are additive. Consumers treat an unrecognized reason as the failure of its Connect code. Requests are not idempotent: each invocation is a new execution that can be charged.

## Storage

Wire messages are not a database schema, and the service stores neither requests nor results. The server contract owns persistence.

## Security

Credentials travel only in the `Authorization` header, never in message fields. Submitted texts, schemas and results must not appear in `ErrorDetail`, error messages or telemetry. Browsers are not supported callers, and API key secrets must stay in server-side code.

## Logging

Correlate failures through `request_id`, which is also the `X-Delitools-Request-Id` response header. Logs never include message contents or credentials.

## Build and Test

Use the root Buf generation, lint, formatting, breaking and freshness checks under the [repository workflow](repository-workflow-contract.md); `protos/scripts/` owns shared Buf entry resolution and breaking checks. Root `pnpm proto:generate` generates Go and TypeScript bindings together and must preserve schemas and instruction files. The root `buf.gen.yaml` input selection includes `protos/delitools/v1`. Run `go build ./...` and `go vet ./...` for the generated Go package. Server integration against mocked Gateway and delcore services lives in `servers/delitools-api`. The Go consumer example `examples/delitools-client` reads a ProtoJSON request file with `DELITOOLS_URL` and `DELITOOLS_TOKEN`, calls `Translate` or `Extract`, and prints the result or the stable error reason; run `go test ./...` and `go build ./examples/delitools-client` from the repository root.

## Dependencies and Integrations

Connect-Go and Protobuf use the root pinned versions. The server calls the [delcore protocol](protos-delcore-v1-contract.md) (`GetCurrentUser`, `Authorize`, `AuthorizeApiKey`, `GetCatalog`, `GetTenantStates`, `IngestEvent`) as a registered consumer service.

## Change Triggers

Update this protocol contract, the [server contract](servers-delitools-api-contract.md) and the [project index](project-delitools.md) when public interfaces change. Update the public documentation for user-visible changes. Describe interface changes and validation results in the pull request.

## References

- [Repository defaults](repository-defaults.md)
- [Project index](project-delitools.md)
- [Server contract](servers-delitools-api-contract.md)
