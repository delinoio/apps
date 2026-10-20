# Project: delcore

## Goal

Provide shared tenants, authorization, metering and tenant-wide billing for independently hosted Delino services.

## Project ID

`delcore`

## Official API Server

The official API server base URL is `https://delcore.api.delino.io`.

## Domain Ownership Map

- `servers/delcore-api`: one Go API and background worker process, migrations and OCI packaging.
- `protos/delcore/v1`: Connect schemas, generated Go and TypeScript messages and Connect bindings.
- `examples/delcore-client`: Go consumer example.

See the [repository boundary contract](repository-boundary-contract.md) for coordinated changes.

## Domain Contract Documents

- [Server contract](servers-delcore-api-contract.md)
- [Protocol contract](protos-delcore-v1-contract.md)

## Cross-Domain Invariants

PostgreSQL is authoritative. Logto owns identities; delcore owns tenant authorization. Paddle owns payment execution; delcore owns usage rating and credits. Purchased credits pay usage only. Paid periods follow Paddle renewal periods, and paid authorization ends at the confirmed period end until a renewal advances it; free periods use UTC calendar months. Consumers fail closed for new paid execution when authorization cannot be checked and retain pending usage reports. Server files use FSL-1.1-ALv2; protocol and consumer files use Apache-2.0.

## Change Policy

When the boundary changes, update the local protocol and server contracts and this project index in the same change. Describe interface effects and client/server integration validation in the pull request. Update instruction files only for ownership or development-rule changes. Record validation and remaining limits in PRs, issues or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Environment contract](repository-environment-contract.md)
- [Workflow contract](repository-workflow-contract.md)

## DeliTools consumer integration

The service created with the reserved identifier `delitools` uses call-time public Gateway prices for all customers, including existing subscriptions. Its trusted usage report carries actual token quantities and an immutable initial-request price snapshot. Delcore computes exact USD token cost with 1.15 translation or 1.30 extraction markup; Gateway credits and discounts do not change the basis. No floating-point or estimated usage is accepted. Existing period rounding, credits, blocking, service-scoped reporting and settlement remain authoritative. Other services retain their subscription price behavior. DeliTools token charges do not consume unit allowances.

DeliTools may authorize tenant-owned service keys independently of the issuing member. Personal-key management requires current membership; service-key management requires current Owner/Admin. User execution still requires the requested capability. These modes are restricted to the registered DeliTools service and do not grant platform authority.

DeliTools deletes pending delivery and unknown-usage records 30 days after their first creation, regardless of state. This explicit exception permits permanent loss of unreported charges; it never deletes delcore's accepted events, replay protection or ledger entries. Other consumers retain pending reports under their existing contracts.

The reserved `delitools` identifier is bound to its service ID at creation and is not a mutable display name. `PutService` rejects renaming that service or renaming another existing service into this identity. The unique identifier cannot be reassigned; there is no service-deletion API. Other service names retain their existing update behavior.
