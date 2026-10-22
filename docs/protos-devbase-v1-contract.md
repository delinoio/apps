# DevBase v1 protocol contract

## Scope

This contract belongs to `delinoio/apps`. The schema and generated Go and TypeScript messages live together in `protos/devbase/v1`. Connect-Go bindings live in its `devbasev1connect` subdirectory. The server behavior is owned by the [server contract](servers-devbase-api-contract.md).

## Runtime and Language

Protobuf package `devbase.v1` with service `DevBaseService`. Generated Go clients and handlers use the root `github.com/delinoio/apps` module; Go consumers use them directly. TypeScript bindings are exported by the private `@delino/protos` workspace as `@delino/protos/devbase/v1/devbase_pb`. There is no REST surface, handwritten SDK or published client package.

## Users and Operators

Developers using Delino login tokens, applications using project API keys, and operators.

## Interfaces and Contracts

`DevBaseService` exposes these Connect RPCs:

- Projects: `CreateProject`, `GetProject`, `ListProjects`, `UpdateProject`, `DeleteProject`.
- Project API keys: `IssueApiKey`, `GetApiKey`, `ListApiKeys`, `RevokeApiKey`.
- End users: `CreateEndUser`, `GetEndUser`, `LookupEndUser`, `ListEndUsers`, `UpdateEndUser`, `DeleteEndUser`.

Conventions:

- Authenticate with `Authorization: Bearer <token>`: a Delino login token or a `dbk_` project API key. The credential-to-operation matrix is in the server contract.
- Every mutation request has `idempotency_key`. Replays within 24 hours return the original result; the same key with different input, a different operation or a different project fails with `already_exists`. One exception follows from deletion: when the data that a create or update produced is later deleted, the server keeps only routing data for that record, so replaying the same key for the same operation, tenant and project returns `not_found` whatever the input and never restores the data.
- Entity IDs are UUID v7 strings. Timestamps use `google.protobuf.Timestamp`. Versions are `int64` values that start at 1; update and delete requests carry the current version and a stale value fails with `aborted`.
- Lists use `page_size` (default 50, maximum 100), an opaque `page_token` and `next_page_token`. No response carries a total count.
- `EndUser.metadata` is a `google.protobuf.Struct`, so a present value is always a JSON object. An absent `metadata` field is the same as an empty object: on create it stores `{}`, and in an update whose `update_mask` contains `metadata` it clears the metadata. An absent field and an explicitly empty object mean the same, but they are different wire messages and therefore have different idempotency fingerprints; reusing one idempotency key for both returns `already_exists`. Metadata strings and object keys must not contain U+0000; such requests fail with `invalid_argument`. `UpdateEndUserRequest.update_mask` selects `external_id`, `name`, `email` and `metadata`.
- `IssueApiKeyResponse.secret` is set only by the initial issuance.
- Every response carries `Metadata.trace_id`. Every error produced by a handler carries an `ErrorDetail` with `ErrorReason`, the trace ID and, for rate limits, `retry_after`. One transport-level exception exists: the transport rejects a message larger than 256 KiB while it reads the message, before any handler runs. That rejection is a plain Connect `resource_exhausted` error with no `ErrorDetail` and no `X-Devbase-Trace-Id` header. Requests between 64 KiB and 256 KiB reach the handler and fail with the typed `invalid_argument`.
- Connect codes: `unauthenticated`, `permission_denied`, `not_found`, `invalid_argument`, `already_exists`, `aborted`, `resource_exhausted` and `unavailable`.

Preserve field numbers and enum values. Add fields and RPCs compatibly; never reuse a removed number.

## Storage

Wire messages are not the database schema. The server contract owns storage, replay retention and deletion behavior.

## Security

Messages never carry key hashes. The only message that carries a key secret is the initial `IssueApiKeyResponse`. Do not log request or response payloads that contain names, email addresses, metadata or external user IDs.

## Logging

Correlate responses, errors and logs through the trace ID without exposing credentials or personal payloads.

## Build and Test

`protos/scripts/` owns shared Buf entry resolution and comparison-revision breaking checks under the [repository workflow](repository-workflow-contract.md).

Use the root Buf generation, lint, formatting, breaking and freshness checks: `pnpm proto:generate`, `pnpm proto:lint` and `buf breaking` against the comparison revision. Generation preserves schemas and instruction files in the shared output tree. Remove obsolete generated files when deleting or renaming schemas. The examples in `examples/devbase-client` and `examples/devbase-client-ts` consume the generated bindings; build and type-check them after protocol changes with `go build ./examples/devbase-client` and `pnpm --filter devbase-client-ts typecheck`. Validate both examples against a local server with a mocked Delcore. Server contract tests run against disposable PostgreSQL and a mocked Delcore.

## Dependencies and Integrations

Connect-Go and Protobuf use the root pinned versions. DevBase is part of the existing Buf input selection.

## Change Triggers

Update this contract, the [server contract](servers-devbase-api-contract.md) and the [project index](project-devbase.md) when public interfaces change, and validate affected server and example integration in the same change.

## References

- [Repository defaults](repository-defaults.md)
- [Server contract](servers-devbase-api-contract.md)

## Rust bindings

`servers/crates/delino-protos` owns the FSL-1.1-ALv2 Rust protocol package in the server workspace. Its Cargo build script uses pinned `connectrpc-build` and the existing schemas with `protoc`; generated messages and service traits stay in its Cargo output directory. The schemas and checked-in Go/TypeScript bindings remain unchanged. Rust server crates depend on this local package. Cargo build and test checks compile these bindings; the existing Buf generation and freshness checks continue to own checked-in Go/TypeScript outputs.
