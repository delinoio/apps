# Project: delcore

## Goal

Provide shared tenants, authorization, metering and tenant-wide billing for independently hosted Delino services.

## Project ID

`delcore`

## Official API Server

The official API server base URL is `https://delcore.api.delino.io`.

## Domain Ownership Map

- `servers/Cargo.toml` and `servers/crates/`: independent server Rust workspace and shared server modules for incremental ports; see the [server Rust workspace contract](servers-rust-workspace-contract.md).

- `servers/scripts/dev-api.mjs`: shared Rust API development launcher.
- `protos/scripts/`: shared Buf entry resolution and protocol breaking checks.

- `servers/delcore-api`: one Rust API and background worker process, migrations and OCI packaging. The `delino` release publishes its image; see the [workflow contract](repository-workflow-contract.md#server-image-publication).
- `servers/delcore-api/catalog`: split-file development catalog examples; the server contract owns the file format and startup behavior.
- `protos/delcore/v1`: Connect schemas, generated Go and TypeScript messages and Connect bindings.
- `examples/delcore-client`: Go consumer example.

See the [repository boundary contract](repository-boundary-contract.md) for coordinated changes.

## Domain Contract Documents

- [Server contract](servers-delcore-api-contract.md)
- [Protocol contract](protos-delcore-v1-contract.md)

## Cross-Domain Invariants

PostgreSQL is authoritative. Logto owns identities; delcore owns tenant authorization. Paddle owns payment execution; delcore owns usage rating and credits. Purchased credits pay usage only. Paid periods follow Paddle renewal periods, and paid authorization ends at the confirmed period end until a renewal advances it; free periods use UTC calendar months. Consumers fail closed for new paid execution when authorization cannot be checked and retain pending usage reports. Registered services read billing-independent tenant state (`ACTIVE`, `BLOCKED`, `DELETING`, `DELETED`, `UNKNOWN`) through `GetTenantStates`; only `DELETED` permits consumer cleanup of retained tenant data. Server files use FSL-1.1-ALv2; protocol and consumer files use Apache-2.0.

Catalog entities use stable, scoped keys and UUID v7 identities. Billing and feature definitions have immutable semantic versions, and equivalent content reuses its version. A tenant selects versions when its subscription changes; a renewal retains those selections, and catalog edits never move existing subscriptions. Meter definitions never change in place; a changed definition needs a new meter key. The [server contract](servers-delcore-api-contract.md#catalog-version-pinning) owns catalog selection and payment-confirmation rules, and the [protocol contract](protos-delcore-v1-contract.md) owns the public types.

Registered services report flat events through `IngestEvent` and correct them with revision compare-and-set through `CorrectEvent`. Acceptance durably stores the event and its processing work; group aggregates update asynchronously. The [server contract](servers-delcore-api-contract.md#event-ingestion-and-asynchronous-metering) owns admission bounds, processing, recovery, and retention.

`Authorize` optionally evaluates a feature against the tenant's pinned plan and add-on versions after the unchanged permission and billing checks. Metered features block once the last applied group aggregate of the current billing period reaches the limit. Each group has its own limit, while rating shares included usage once per tenant meter period and clamps negative groups to zero ([comparison](servers-delcore-api-contract.md#group-limits-and-shared-included-usage)); authorization reserves nothing, so concurrent or lagging execution can overspend. `ListMeterAggregates` and `GetMeteringStatus` expose applied aggregates, processing lag, and failures. The [server contract](servers-delcore-api-contract.md#feature-authorization-and-metering-queries) owns these rules.

Catalog reads identify the active immutable price version for each stable price key so subscription clients can select the version accepted by the server.

Development startup reconciles the declared split-file YAML catalog after schema compatibility and single-instance checks, before API and worker startup. Root, service-local, and Portless development use that path; YAML edits need an API restart. Production startup does not read the development catalog, and standalone catalog commands are out of scope. The [server contract](servers-delcore-api-contract.md#development-catalog) owns the file format, examples, and reconciliation rules.

The catalog and event metering change is a coordinated pre-release transition: the server and its repository callers change together without a feature flag, no production data is migrated, development databases may be reset, and migrations stay explicit and separate from catalog synchronization. The [server operations section](servers-delcore-api-contract.md#catalog-and-metering-operations) owns the transition and recovery procedures.

## Change Policy

When the boundary changes, update the local protocol and server contracts and this project index in the same change. Describe interface effects and client/server integration validation in the pull request. Update instruction files only for ownership or development-rule changes. Record validation and remaining limits in PRs, issues or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)
