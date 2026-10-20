# DeliTools v1 protocol contract

## Scope

Schemas and generated Go/TypeScript bindings live in `protos/delitools/v1`; Connect Go bindings live in `delitoolsv1connect`. No separate SDK is published.

## Runtime and Language

Connect RPC, Protobuf, root Go module and private `@delino/protos` exports.

## Users and Operators

External clients use Logto bearer tokens or DeliTools bearer API keys. Key management requires a Logto user token. Operator RPCs require a dedicated platform role.

## Interfaces and Contracts

`DeliToolsService` exposes synchronous `TranslateTextArray` and `ExtractDataFromText`, key creation/list/revocation, operator settings and request inspection. Success returns only a request ID and result. Requests carry tenant IDs and optional idempotency keys. Duplicate state never replays a result. Authentication precedes duplicate disclosure.

Schema subset: object roots, nested object/array/string/number/integer/boolean/null types, required properties, `additionalProperties: false`, descriptions and scalar enums. All object properties must be required. Reject unknown schema keywords and recursive references before execution. Missing source information is extraction-impossible; malformed or nonconforming output is extraction-failed. No repair retry is permitted.

Errors carry `ErrorDetail` with a stable reason and UUID v7 request ID, an English message, and a Connect code. Invalid input/schema/model maps to invalid_argument; credentials to unauthenticated; permissions to permission_denied; billing and absent prices to failed_precondition; configured limits to resource_exhausted; processing to aborted; completed duplicates to already_exists; input conflicts to failed_precondition; unknown outcomes to unknown; impossible extraction to failed_precondition; malformed extraction/provider output to data_loss; provider failures to unavailable; timeouts to deadline_exceeded; cancellation to canceled; internal storage failure to internal.

Operator settings are mandatory and validated, including explicit AI retry count (zero is valid). Catalog updates never enable models. Requests freeze their initial catalog snapshot for all attempts. Platform mutations require a reason; tenant administrators have no platform authority.

## Storage

Wire messages are not persistence objects. Never persist source, schema or result messages. State inspection contains only safe state, usage attribution, pricing and attempt metadata. Deduplication is guaranteed for 30 days from creation, across credentials within tenant and method; expiry permits a fresh execution.

## Security

Current delcore permissions and billing checks apply to both credential types. Service keys are tenant-owned; personal keys intersect current user permissions with their capabilities. Secrets are returned once and stored only as verification hashes. Revocation blocks new calls and retries, while already-running calls may finish and remain billable.

## Logging

Use safe correlation IDs and bounded reason codes. No raw message logging.

## Build and Test

Run root Buf lint, generation, freshness, FILE breaking checks and protocol Go tests. The server and consumer example currently have reserved skeletons. Once their implementations are present, also run consumer Go tests and verify server behavior with PostgreSQL and mocked providers in this checkout.

## Dependencies and Integrations

Use the root Go module and pinned protobuf/Connect versions. Public docs live in `apps/delino-docs`; internal implementation contracts remain in root `docs/`.

## Change Triggers

Update the server contract and project index for interface changes. Preserve field numbers and enum values.

## References

- [Project](project-delitools.md)
- [Server](servers-delitools-api-contract.md)
- [Defaults](repository-defaults.md)
