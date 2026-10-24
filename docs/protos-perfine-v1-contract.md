# Perfine v1 protocol and evidence

## Scope

`protos/perfine/v1` owns canonical Connect schemas and generated Go/TypeScript bindings. This contract also owns the shared version and resource rules for CLI JSON, configuration, MCP, and evidence manifests; their implementation remains in the owning component.

## Runtime and Language

Protobuf and Connect RPC. Generate server Rust bindings through the existing server protocol crate into Cargo output. Keep Go consumers in the root `github.com/delinoio/apps` module and TypeScript exports in `@delino/protos`; do not duplicate generated source in apps or servers.

## Users and Operators

CLI/MCP clients, dashboard, Action uploader, API, and external protocol consumers.

## Interfaces and Contracts

- Version configuration, JSON, evidence schemas, MCP contracts, and RPC API from v1. Add fields compatibly, reject unsupported major versions explicitly, and use a new major version with migration guidance for breaking changes.
- Connections, experiments, candidates, approvals, and uploads are distinct resources with UUID v7 IDs. Keep GitHub IDs and commit SHAs separate. Mutations carry request IDs, lists use cursor pagination, and responses/errors expose request IDs.
- Evidence manifests include schema version, source and harness SHAs, tools/environment, input hashes, policy, metric units, file hashes, and completion status. Preserve enough information to independently assess a result and reproduce its declared conditions.

Execution state is `queued`, `running`, `completed`, `failed`, or `cancelled`. Performance verdict is independently `pass`, `regression`, `inconclusive`, or `not_evaluated`. A completed execution may have no candidate. Preserve exceptions and approval requirements independently of those enums.

CLI, MCP, and hosted comparison use the same evidence and evaluation semantics from [Engine contract](crates-perfine-core-contract.md). Mutating operations cover connection, experiment lifecycle, evidence upload, consent, approval/publication, and deletion with the independent permissions in [Api contract](servers-perfine-api-contract.md).

## Storage

Manifests preserve schema version, source/harness identities, inputs, tools/environment, policy, metric units, file hashes, and completion status. Local storage follows [Cli contract](cmds-perfine-cli-contract.md); hosted evidence eligibility, retention, tombstones, and signed URLs follow [Api contract](servers-perfine-api-contract.md). Request deduplication expires 30 days after creation; retained publication and deletion records remain independent safeguards.

## Security

Never define AI credential fields or a source/diff storage API. Keep UUID v7 resource IDs distinct from GitHub IDs and commit SHAs. Server-side authorization must validate space, repository, and execution scope; request IDs do not confer authority.

## Logging

Responses and errors expose request IDs. Structured logs must not expose API keys, AI credentials, raw evidence, or user state.

## Build and Test

With schemas implemented, run root `pnpm proto:lint`, `pnpm proto:generate`, generation freshness checks, and `node protos/scripts/check-breaking.mjs "<comparison-revision>"`. Run affected Go vet/tests/consumer builds and Rust/frontend consumers under the repository boundary contract. Test unsupported majors, additive compatibility, malformed messages, cursor pagination, identifiers, immutable inputs, missing limits, manifest completeness, and separated execution/verdict state.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Engine contract](crates-perfine-core-contract.md), [Cli contract](cmds-perfine-cli-contract.md), [Api contract](servers-perfine-api-contract.md), [Web contract](apps-perfine-web-contract.md), and [repository boundary](repository-boundary-contract.md).

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)
