# Project: delcore

## Goal

Provide shared tenants, authorization, metering and tenant-wide billing for independently hosted Delino services.

## Project ID

`delcore`

## Official API Server

The official API server base URL is `https://delcore.api.delino.io`.

## Domain Ownership Map

- `.github/workflows/release.yml` and reusable release jobs: changeset-selected web and server image publication; see the [release contract](repository-workflow-contract.md#project-release-preparation).

- `servers/Cargo.toml` and `servers/crates/`: independent server Rust workspace and shared server modules for incremental ports; see the [server Rust workspace contract](servers-rust-workspace-contract.md).

- `servers/scripts/dev-api.mjs`: shared Rust API development launcher.
- `protos/scripts/`: shared Buf entry resolution and protocol breaking checks.

- `servers/delcore-api`: one Rust API and background worker process, migrations and OCI packaging. The changeset release publishes its image; see the [workflow contract](repository-workflow-contract.md#server-image-publication).
- `servers/delcore-api/catalog`: split-file development catalog examples; the server contract owns the file format and startup behavior.
- `protos/delcore/v1`: Connect schemas, generated Go and TypeScript messages and Connect bindings.
- Registered consumer services such as DeliTools (`servers/delitools-api`, `protos/delitools/v1`) use Delcore only through its Connect API. Delcore stays service-neutral and holds no consumer-specific runtime or protocol logic.
- `examples/delcore-client`: Go consumer example.

See the [repository boundary contract](repository-boundary-contract.md) for coordinated changes.

## Domain Contract Documents

- [Server contract](servers-delcore-api-contract.md)
- [Protocol contract](protos-delcore-v1-contract.md)

## Cross-Domain Invariants

PostgreSQL is authoritative. Logto owns identities; delcore owns tenant authorization. Paddle owns payment execution; delcore owns usage rating and credits. Purchased credits pay usage only. Paid periods follow Paddle renewal periods, and paid authorization ends at the confirmed period end until a renewal advances it; free periods use UTC calendar months. Consumers fail closed for new paid execution when authorization cannot be checked and retain pending usage reports for delivery and reconciliation until 30 days after first creation. Delete these consumer records at expiry regardless of processing state; unresolved usage is abandoned without estimated billing. Registered services read billing-independent tenant state (`ACTIVE`, `BLOCKED`, `DELETING`, `DELETED`, `UNKNOWN`) through `GetTenantStates`; only `DELETED` permits consumer cleanup caused by tenant deletion. Fixed record expiry applies independently of tenant state. Server files use FSL-1.1-ALv2; protocol and consumer files use Apache-2.0.

Catalog entities use stable, scoped keys and UUID v7 identities. Billing and feature definitions have immutable semantic versions, and equivalent content reuses its version. A tenant selects versions when its subscription changes; a renewal retains those selections. Usage unit prices are resolved separately at the initial execution start for every customer, including existing subscriptions; catalog edits apply to later executions without changing selected features, included-usage grants or recurring payment prices. Meter definitions never change in place; a changed definition needs a new meter key. The [server contract](servers-delcore-api-contract.md#catalog-version-pinning) owns catalog selection and payment-confirmation rules, and the [protocol contract](protos-delcore-v1-contract.md) owns the public types.

Registered services report flat events through `IngestEvent` and correct them with revision compare-and-set through `CorrectEvent`. Acceptance durably stores the event and its processing work; group aggregates update asynchronously. The [server contract](servers-delcore-api-contract.md#event-ingestion-and-asynchronous-metering) owns admission bounds, processing, recovery, and retention.

`Authorize` optionally evaluates a feature against the tenant's pinned plan and add-on versions after the unchanged permission and billing checks. Metered features block once the last applied group aggregate of the current billing period reaches the limit. Each group has its own limit, while rating shares included usage once per tenant meter period and clamps negative groups to zero ([comparison](servers-delcore-api-contract.md#group-limits-and-shared-included-usage)); authorization reserves nothing, so concurrent or lagging execution can overspend. `ListMeterAggregates` and `GetMeteringStatus` expose applied aggregates, processing lag, and failures. The [server contract](servers-delcore-api-contract.md#feature-authorization-and-metering-queries) owns these rules.

Catalog reads identify the active immutable price version for each stable price key so subscription clients can select the version accepted by the server. Append-only activation history resolves usage prices at execution start, including A→B→A reactivation. Late delivery, retries and corrections preserve that price attribution.

Development startup reconciles the declared split-file YAML catalog after schema compatibility and single-instance checks, before API and worker startup. Root, service-local, and Portless development use that path; YAML edits need an API restart. Production startup does not read the development catalog, and standalone catalog commands are out of scope. The [server contract](servers-delcore-api-contract.md#development-catalog) owns the file format, examples, and reconciliation rules.

Catalogs and event metering use only current definitions and pinned selections; retired quantity APIs and runtime/data compatibility paths are removed. The catalog and event metering change is a coordinated pre-release transition: the server and its repository callers change together without a feature flag, no production data is migrated, development databases may be reset, and migrations stay explicit and separate from catalog synchronization. The [server operations section](servers-delcore-api-contract.md#catalog-and-metering-operations) owns the transition and recovery procedures.

Delcore provides reusable personal and tenant service API keys for any registered service. A key has explicit tenant attribution and selected service and capability grants, and `AuthorizeApiKey` lets only the calling registered service verify and authorize its own grants. Users manage their own personal keys, and tenant Owners and Admins manage the tenant's service keys, with Logto user tokens. The secret is shown once and only its hash is stored; it is never logged or replayed. Grants and expiry are fixed at creation, and a replacement key plus revocation changes them. Personal-key execution requires the intersection of its grants and the user's current tenant permissions; tenant-key execution uses its grants independently of the issuing user's membership; both require current tenant and billing authorization. Key validity does not depend on 30-day retention. The [server contract](servers-delcore-api-contract.md#api-keys) and [protocol contract](protos-delcore-v1-contract.md#api-keys) own these rules.

Request deduplication records, consumer usage-delivery/reconciliation records, platform audit records and operational logs expire 30 days after their first creation. Retries and state changes do not extend expiry; unresolved records have no retention hold. This policy applies across services, not only to DeliTools. Accepted usage identities, delcore processing work and accounting entries retain their separate accounting and replay-protection policies.

## Change Policy

When the boundary changes, update the local protocol and server contracts and this project index in the same change. Describe interface effects and client/server integration validation in the pull request. Update instruction files only for ownership or development-rule changes. Record validation and remaining limits in PRs, issues or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)
