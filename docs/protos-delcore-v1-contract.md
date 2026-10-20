# Delcore v1 protocol contract

## Scope

This contract belongs to `delinoio/apps`. Schemas and generated Go and TypeScript messages live together in `protos/delcore/v1`. Connect-Go bindings live in its `delcorev1connect` subdirectory.

## Runtime and Language

Protobuf package `delcore.v1`; generated Go clients and handlers use the root `github.com/delinoio/apps` module, consumed directly by local server packages in the same module. There is no separate SDK package or UI.

## Users and Operators

User clients, service-specific M2M clients and platform operators.

## Interfaces and Contracts

Expose typed Connect interfaces for user discovery, tenants, memberships, invitations, ownership transfer, groups, catalog, subscriptions, top-ups, usage, corrections, ledger, audit, failed-job recovery and service tenant state. Every protected request checks current permissions. Mutations accept client-generated idempotency keys; repeated keys with different inputs fail. Enum values describe states, roles and failure reasons. Delcore entity IDs are UUID v7; provider IDs remain explicit external references. Decimal quantities and monetary values use canonical decimal strings, never floats. Public decimal inputs are limited to 64 characters; internal products and sums retain their full precision. Return correlation metadata on responses and errors. Lists have bounded pagination.

Catalog mutations reject invalid combinations as invalid-argument errors. `PutPlan` rejects a free plan that enables postpaid continuation, including the default free plan, because free tenants never have a Paddle subscription to collect postpaid usage. `CreatePrice` rejects an add-on price whose service differs from the service that owns the add-on, and a price that names an unknown add-on, before it writes any price or audit row. These checks apply to new requests only; existing plans and prices are not rewritten.

Occurrence timestamps use PostgreSQL microsecond precision. The service normalizes timestamps before hashing, storing and matching corrections. Prorated included quantities use a 0.000001 quantum; monetary products retain exact decimal precision until currency rounding.

Deletion and withdrawal return a failed precondition while a previously attempted subscription intent remains incomplete. Unattempted subscription intents, and intents that the provider definitively rejected without creating a subscription, are stopped atomically when deletion starts. A stopped rejected intent keeps its rejection category and cannot be retried.

Disabling automatic top-ups clears the settings and stops queued charges that never reached the provider in the same request. Charges that may have reached the provider, including a rejected charge that an operator retried, continue. Accepting a subscription cancellation stops the same unattempted charges.

Negative postpaid corrections carry forward against usage charges. Operators refund any remainder in Paddle on cancellation. Confirmed usage-line refunds clear the corresponding settlement balance; fixed subscription refunds do not clear usage debt.

Consumer services with the `authorize` scope may call `GetTenantStates` for 1 to 100 known tenant UUIDs. The response repeats each requested ID, in request order, with one `TenantState`: `TENANT_STATE_ACTIVE`, `TENANT_STATE_BLOCKED`, `TENANT_STATE_DELETING`, `TENANT_STATE_DELETED` or `TENANT_STATE_UNKNOWN`. `TENANT_STATE_UNSPECIFIED` is the proto3 zero value and is never returned. Overlapping conditions resolve as `DELETED` over `DELETING` over `BLOCKED` over `ACTIVE`. `BLOCKED` means operator blocking only; subscription expiry, renewal failure, dispute and balance conditions never change the state. `DELETING` means deletion is pending, `DELETED` means deletion is final, and an unknown tenant is `UNKNOWN`. The call is read-only, needs no user ID, and returns no tenant names, user details or billing data beyond the standard response metadata. Invalid IDs, an empty request or more than 100 IDs return `ERROR_REASON_INVALID_INPUT`; user tokens, unregistered services and services without the `authorize` scope return `ERROR_REASON_FORBIDDEN`.

Consumers treat `UNKNOWN`, request errors and unavailable state as no permission for access or destructive cleanup. Only an explicit `DELETED` permits automatic cleanup of the consumer's retained tenant data; `BLOCKED` and `DELETING` do not. A consumer may use `BLOCKED` and `DELETING` to deny project-scoped service calls while applying its own documented management exceptions. `Authorize` and `CheckTenantLifecycle` semantics, field numbers and enum values are unchanged.

Consumer services with the `authorize` scope may call `CheckTenantLifecycle` for up to 100 known tenant IDs. Only IDs whose deletion is finalized are returned. Unknown IDs and pending deletion do not authorize cleanup. The result contains no tenant names or personal information. Consumers fail closed on request errors and delete their own retained tenant data only after explicit finalization.

## Storage

Wire messages are not the database schema. Historical pricing and usage identities survive replay and corrections. Server retention boundaries are owned by the [server contract](servers-delcore-api-contract.md). Validate changes that affect retention against the local server implementation and this wire contract.

## Security

User and M2M credentials cannot substitute for each other. Service reporting is scoped to the authenticated registered service. Operator changes require an audit reason. Consumer application URLs are operator-allowlisted. Payment callbacks do not authorize entitlements. Enabling automatic top-ups requires an active Paddle subscription; manual top-ups remain available without a subscription. Subscription cancellation suspends automatic top-ups; settings cannot re-enable them while cancellation is pending.

## Logging

Correlate responses and logs without exposing credentials or raw provider payloads.

## Build and Test

Use the root Buf generation, lint, formatting, breaking and freshness checks. Generation must preserve schemas and instruction files in the shared output tree. Remove obsolete generated files when deleting or renaming schemas. The consumer example lives in `examples/delcore-client`; run `go test ./...` and `go build ./examples/delcore-client` here. Connect service contract tests against real PostgreSQL and mocked external services live under `servers/` in this checkout. Run affected server integration checks and report any unperformed checks in the pull request. Root `pnpm proto:generate` also generates TypeScript bindings beside the schemas. Git Tracker consumes them through the private `@delino/protos` workspace; do not publish a separate TypeScript SDK.

## Dependencies and Integrations

Connect-Go and Protobuf use the root pinned versions. Add delcore to the existing Buf input selection.

## Change Triggers

Update this protocol contract and the [project index](project-delcore.md) when public interfaces change. Describe the interface change and validation results in the pull request. Update the local server contract and validate affected integration in the same change. Preserve field numbers and enum values.

## References

- [Repository defaults](repository-defaults.md)
- [Server contract](servers-delcore-api-contract.md)
