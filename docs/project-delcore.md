# Project: delcore

## Goal

Provide shared tenants, authorization, metering and tenant-wide billing for independently hosted Delino services.

## Project ID

`delcore`

## Official API Server

The official API server base URL is `https://delcore.api.delino.io`.

## Domain Ownership Map

- `servers/delcore-api` in `delinoio/servers`: one Go API and background worker process, migrations and OCI packaging.
- `protos/delcore/v1` in `delinoio/apps`: Connect schemas, generated Go messages and Connect bindings.
- `examples/delcore-client` in `delinoio/apps`: Go consumer example.

See the [repository boundary contract](repository-boundary-contract.md) for coordinated changes.

## Domain Contract Documents

- Private reference for maintainers with servers access: [Server contract in servers](https://github.com/delinoio/servers/blob/main/docs/servers-delcore-api-contract.md)
- [Protocol contract](protos-delcore-v1-contract.md)

## Cross-Domain Invariants

PostgreSQL is authoritative. Logto owns identities; delcore owns tenant authorization. Paddle owns payment execution; delcore owns usage rating and credits. Purchased credits pay usage only. Paid periods follow Paddle renewal periods, and paid authorization ends at the confirmed period end until a renewal advances it; free periods use UTC calendar months. Consumers fail closed for new paid execution when authorization cannot be checked and retain pending usage reports. Repository-owned code uses Apache-2.0.

## Change Policy

When the boundary changes, contributors update the apps-local protocol contract and this project index and describe the interface change in their apps pull request. Maintainers with servers access coordinate the private server contract update and server integration validation before adopting the apps revision. External contributors do not need to read or edit private contracts. Update instruction files only for ownership or development-rule changes. Record validation and remaining limits in PRs, issues or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)
