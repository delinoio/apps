# Project: delcore

## Goal

Provide shared tenants, authorization, metering and tenant-wide billing for independently hosted Delino services.

## Project ID

`delcore`

## Official API Server

The official API server base URL is `https://delcore.api.delino.io`.

## Domain Ownership Map

- `servers/scripts/dev-api.mjs`: shared Go API development launcher.
- `protos/scripts/`: shared Buf entry resolution and protocol breaking checks.

- `servers/delcore-api`: one Go API and background worker process, migrations and OCI packaging. The `delino` release publishes its image; see the [workflow contract](repository-workflow-contract.md#server-image-publication).
- `protos/delcore/v1`: Connect schemas, generated Go and TypeScript messages and Connect bindings.
- `examples/delcore-client`: Go consumer example.

See the [repository boundary contract](repository-boundary-contract.md) for coordinated changes.

## Domain Contract Documents

- [Server contract](servers-delcore-api-contract.md)
- [Protocol contract](protos-delcore-v1-contract.md)

## Cross-Domain Invariants

PostgreSQL is authoritative. Logto owns identities; delcore owns tenant authorization. Paddle owns payment execution; delcore owns usage rating and credits. Purchased credits pay usage only. Paid periods follow Paddle renewal periods, and paid authorization ends at the confirmed period end until a renewal advances it; free periods use UTC calendar months. Consumers fail closed for new paid execution when authorization cannot be checked and retain pending usage reports. Registered services read billing-independent tenant state (`ACTIVE`, `BLOCKED`, `DELETING`, `DELETED`, `UNKNOWN`) through `GetTenantStates`; only `DELETED` permits consumer cleanup of retained tenant data. Server files use FSL-1.1-ALv2; protocol and consumer files use Apache-2.0.

Catalog entities use stable, scoped keys and UUID v7 identities. Billing and feature definitions have immutable semantic versions. A tenant selects versions when its subscription changes; a renewal retains those selections. The [server contract](servers-delcore-api-contract.md) owns catalog selection and payment-confirmation rules, and the [protocol contract](protos-delcore-v1-contract.md) owns the public types.

Catalog reads identify the active immutable price version for each stable price key so subscription clients can select the version accepted by the server.

## Change Policy

When the boundary changes, update the local protocol and server contracts and this project index in the same change. Describe interface effects and client/server integration validation in the pull request. Update instruction files only for ownership or development-rule changes. Record validation and remaining limits in PRs, issues or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)
