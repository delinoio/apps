# Delcore API contract

Rust port preparation follows the [server Rust workspace contract](servers-rust-workspace-contract.md). This service remains implemented in Go until its individual port. Root development uses process-compose with the existing workspace launcher.

## Scope

`servers/delcore-api` owns the independent service, migrations and background jobs. The consumer example and protocol source live in root `examples/` and `protos/`.

## Runtime and Language

Use the root Go module and pinned toolchain. Run one process against PostgreSQL. Root `pnpm dev` includes this service through its workspace script.

Until the official Go 1.27.3 Bookworm image is published, the OCI build stage bootstraps from `golang:1.27.2-bookworm` with `GOTOOLCHAIN=auto`. After copying `go.mod`, Go selects and downloads the required 1.27.3 toolchain before downloading dependencies or compiling the server. Restore the matching image and remove the override when it becomes available.

## Users and Operators

Authenticated users, registered M2M consumer services, tenant administrators and dedicated platform operators.

## Interfaces and Contracts

The official API server base URL is `https://delcore.api.delino.io`.

**Project and runtime**

- Use the root Go toolchain and module. Run the API and background settlement, retry, notification, and retention jobs in one server process.
- Use PostgreSQL for authoritative data, accounting records, and durable jobs.
- Support one running instance in v1. Provide an OCI image, configuration documentation, explicit database migrations, startup compatibility checks, and backup/update/recovery procedures.
- Do not provision deployment infrastructure as part of this requirement. The `delino` release workflow publishes the image as described in the [workflow contract](repository-workflow-contract.md#server-image-publication).
- Follow repository environment and secret ownership rules. Commit placeholders and safe examples only; require operator-owned credentials for real integrations.

**Identity, tenancy, and authorization**

- Logto owns authentication and user identity. Delcore owns tenants, memberships, groups, roles, and service permissions.
- Validate user access tokens and service-specific Logto M2M tokens. Bind each service identity to its permitted reporting and query operations.
- Create one personal tenant on first authenticated use. Personal tenants belong exclusively to their user.
- Allow users to create organization tenants and join multiple organizations through email invitations.
- Tenants, memberships, and groups are shared across Delino services.
- Support multiple flat groups per organization. Effective group permissions are additive; nested groups and explicit deny policies are excluded.
- Provide fixed roles:

| Role    | Authority                                                            |
| ------- | -------------------------------------------------------------------- |
| Owner   | All tenant administration, billing, ownership transfer, and deletion |
| Admin   | Members, invitations, and groups; no implicit billing authority      |
| Billing | Subscription, payment, and usage administration                      |
| Member  | Granted service use                                                  |

- Allow multiple roles per member. Admin cannot grant Owner, Admin, or Billing roles.
- Each organization has exactly one Owner. Ownership transfer requires recipient acceptance. Prevent the last Owner from leaving before transfer or organization deletion.
- Platform operators use a dedicated Logto role. Require reasons and audit records for operator mutations.
- Enforce tenant boundaries and current permissions on every protected operation. Group permissions grant registered service capabilities.
- Consumer services must reject new paid execution when delcore authorization or billing status cannot be checked. Consumers retain pending usage reports for later delivery.

**Catalog and subscription lifecycle**

- Provide platform operator APIs for services, meters, plans, add-ons, and versioned prices.
- Assign persistent UUID v7 identities to human-readable catalog keys. Service and plan keys are catalog-wide; meter, feature, and add-on keys are service-scoped; price keys are scoped to their owning plan or add-on. A source-file move or an equivalent declaration does not change an identity.
- Keep each meter definition immutable for its key, including its event type, aggregation, value attribute, filters, and group-by fields. A changed definition needs a new meter key. A meter key can be referenced by a feature or a price only within its service.
- Support access, metered-limit, and typed configuration features. Configuration types are boolean, integer, decimal, string, and string list. Validate each plan or add-on value against its feature definition. A metered feature names a meter in the same service. Reject a subscription combination when its base plan and an add-on, or two add-ons, define the same feature.
- Keep active catalog versions separate from tenant selections. Version content includes the plan billing policy, selected price versions and feature values. Normalize decimals and order-independent collections before comparison. Reuse an immutable version for equivalent content, including when the catalog returns from A to B to A; display-name edits do not create billing versions. Operator mutations can select new versions while older tenant selections remain intact. Before a plan or add-on mutation changes a product that tenants use without a selected version, such as a product created before catalog versioning, record the current definition as their selection. Backdate that selection so late usage keeps the recorded policy: a plan selection starts at the earlier of the tenant's current billing period start and its open plan entitlements, and an add-on selection starts at its earliest open add-on entitlement. The start never precedes an earlier selection of the same product or follows the mutation.
- Select the current plan and add-on versions when a tenant is created or an explicit subscription change is requested. When an explicit change selects a plan or add-on that has no version yet, such as a product created before catalog versioning, record its current definition as a version and pin that version. An explicit change must select exactly the catalog price versions of each selected product's active version; legacy price rows without catalog versions are excluded from both sides of that comparison. A pending paid or scheduled change retains its requested version IDs; only provider-confirmed payment or the scheduled free period activates them. Free refresh applies the selected version immediately, and confirmed cancellation selects the then-current default free version. Routine paid renewal keeps the tenant's selected versions, prices and features. Previous selections remain available for historical billing and authorization decisions.
- Reject an add-on price whose service differs from the service that owns the add-on, and reject a price that names an unknown add-on, as invalid arguments. Add-on ownership by a service is immutable, so the check at price creation is sufficient; existing prices are not rewritten.
- Each tenant has one base plan and optional service add-ons, consolidated into tenant-wide monthly billing.
- Support free plans without time-limited trials. New tenants start on the operator-designated default free plan.
- Support USD only, with two decimal places for currency minor units. Fix the currency and minor-unit exponent in code; do not expose deployment settings for either value. Prices and Paddle transactions must use USD. Do not perform currency conversion.
- Support fixed unit price multiplied by measured usage, with included free usage. Exclude tiered rates and custom formulas.
- Operators explicitly rank base plans for upgrade/downgrade classification.
- Apply upgrades after successful immediate prorated payment. Add-ons follow the same immediate prorated addition rule.
- Apply downgrades, add-on removal, and cancellation at the next renewal.
- Preserve existing products and permissions if a requested upgrade payment fails.
- Existing subscriptions retain their price version. New subscriptions and explicit plan changes select the applicable new version.
- Included usage expires each month. An upgrade grants the prorated difference for the remaining period.
- After paid cancellation takes effect, move the tenant to the free plan and end paid add-ons. Preserve prior debt and disputes separately.
- Treat a confirmed cancellation as complete for every cancel intent of the tenant, including a terminally failed one, so a stale failed cancel job cannot hold final deletion. Duplicate and stale provider notifications keep their ordering protection and do not complete jobs.
- Retain the latest subscription status and occurrence time by provider subscription ID even when no tenant is associated yet. When the initial subscription payment establishes the association, apply retained state that occurred after that payment in the same transaction: a canceled state moves the tenant to the free plan and ends paid products, and a past-due state blocks paid use as renewal failure. The payment is ordered by its provider billing time, capped at the processing time, so a payment that reconciliation or the immediate provider response confirms late is compared with retained state by when it happened and its entitlements start at that time. The confirmed initial payment becomes the subscription's ordering watermark, so a delayed notification that occurred before it stays stale in either delivery order. Retain an unresolved past-due notification separately from the latest status, so a later active status or a past-due notification delivered after a newer active status cannot erase it; a retained cancellation outranks it. Older retained state never overrides a newer payment, the newest occurrence wins regardless of delivery order, and an active status alone never grants paid products or clears a payment failure. Retention applies to notifications processed after this behavior ships; delcore does not backfill or reconstruct notifications that were already acknowledged without retained state.
- Never activate paid entitlements or credit balances solely from a checkout request or browser return.

### Development catalog

The default development catalog lives at `servers/delcore-api/catalog`; when the service working directory is `servers/delcore-api`, its relative path is `catalog`. `CATALOG_DIR` overrides it with an absolute path or a path relative to the service working directory. The same environment precedence applies as other settings. Development startup requires an existing directory containing at least one `.yaml` or `.yml` file. Discovery is recursive. Each file has exactly one entity mapping with a `kind` and stable `key`; unknown fields, duplicate kind/scope/key identities (including identical copies), malformed or multi-document YAML, invalid types, missing references, and catalog constraints reject startup. File names and traversal order do not define identity or reference resolution. The committed catalog demonstrates declarations split across files and a nested reference.

The supported kinds are `service`, `meter`, `feature`, `plan`, `addon`, and `price`. Service and plan keys are catalog-wide. Meter, feature, and add-on declarations name a service key, and price declarations name a plan key or an add-on `{service, key}` reference. Feature grants name both service and feature keys. A price names its service and meter keys. References can resolve to declarations in the same file set or retained database entities. Each file is authoritative for the entity it declares. The complete set is validated and applied in one transaction; a failure rolls back every change and stops startup. Omitted entities remain in their last database state; replacing an omitted default free plan is a conflict and requires declaring the previous default in the file set. A later development startup restores declared fields and active definitions changed through operator APIs. Stable UUID v7 identities survive file moves and restarts. Identical billing content reuses its prior version, including A→B→A; display names, file paths, and order do not create a new billing version.

Meter `aggregation` values are `count`, `sum`, `min`, `max`, `avg`, and `unique_count`. The event type, aggregation, value attribute, filters, and group-by list are immutable for a meter key. Filter operators are `equal`, `not_equal`, `in`, `greater_than`, `greater_than_or_equal`, `less_than`, `less_than_or_equal`, and `exists`. Filter values are quoted YAML strings: equality, inequality, and membership compare string scalar values; numeric comparisons use a single canonical decimal string; `exists` takes no values and checks presence of any scalar. Filters combine with AND. Group-by and membership order do not affect the semantic version. Access grants use `enabled: true|false`, metered grants use a nonnegative decimal `limit` string, and configuration grants use `configuration` with the declared boolean, integer, quoted decimal, string, or string-list type. A grant supplies exactly one value.

After explicit migration and a successful schema compatibility check, the server acquires the single-instance lock and applies the YAML set only when `DEVELOPMENT=true`. It completes before listening or processing jobs. Root, service-local, and Portless development use the same API startup path. Air watches Go and SQL, but YAML-only edits do not restart it; restart the API to apply them. Production startup and `-migrate` do not apply YAML. Correct an invalid file or constraint and restart; do not edit the catalog tables directly to work around a failed transaction. Restoring an earlier valid definition selects its old version for future explicit selections without moving existing subscriptions.

**Metering, credits, and settlement**

- Meter definitions match an event type and AND-combined scalar attribute filters. Filters support equality, inequality, membership, numeric comparisons, and existence. Group keys include sorted attribute names, scalar types, and canonical values so equivalent flat events select the same group without key collisions.
- For each matched meter and group, compute count, sum, min, max, average, or exact unique count. Retain the exact sum and count for averages; round the exposed average to six decimal places using half-up rounding, with negative ties rounded away from zero. Negative numeric inputs remain valid. Replacing or cancelling an event recomputes affected groups and removes empty groups.
- Check group and distinct-value cardinality against the complete accepted event state, including events awaiting aggregate materialization. Apply the admission decision to the whole event when multiple meters match.
- Services report measured usage after execution. Delcore calculates charges using its price contracts.
- Preserve usage event identity, service and tenant attribution, occurrence time, and applicable price history.
- Deduplicate accepted usage durably. A service may correct its own prior event through a referenced correction API.
- Keep the ledger append-only. Represent corrections, refunds, and reversals as additional entries.
- Use exact decimal arithmetic without binary floating point. Preserve unrounded credit ledger precision; round monthly service/meter totals to currency minor units using half-up rounding.
- Late usage uses the price applicable when it occurred and becomes an adjustment on the next bill, retaining its original period. Bound accepted lateness through operator configuration. Negative corrections remain durable in the ledger and offset subsequent usage charges, including across restarts. They do not reduce fixed subscription fees. On cancellation, operators refund any remaining usage overpayment in Paddle; synchronize the approved usage-line refund before completing deletion. Invoice-level payment offsets retain their source job and transaction item without inventing service attribution.
- Purchased credits represent monetary balances and do not expire.
- Support manual top-ups for all tenants. Automatic top-ups require an active Paddle subscription; tenants without one use manual top-ups. Automatic top-up settings require a threshold, amount, and monthly cap.
- Permit only one in-flight automatic top-up per tenant. Suspend automatic top-ups after payment failure or an accepted subscription cancellation. Stop unattempted automatic top-up jobs when cancellation is accepted; reconcile any payment already in flight. Disabling automatic top-ups clears the settings and stops queued, unattempted automatic top-up jobs in the same transaction; running, reconciling, previously attempted and externally identified jobs continue, including a job that the provider rejected and an operator explicitly retried. Cancellation and disabling apply the same unattempted rule. A job that the worker deferred inside Paddle's billing window without contacting the provider is unattempted, and a deferred operator retry keeps its rejection category so it stays attempted. Jobs stopped by disabling or cancellation do not count toward the monthly cap. Choosing a tenant for an automatic top-up and queueing its job happen in one maintenance transaction, and every state transition, including settings changes, cancellation and worker claims, runs under the single-instance transaction lock. A settings change therefore either commits before the maintenance pass reads the tenant, so no job is queued, or after the pass commits, so the same change stops the queued job. Integration tests hold the pass between selection and insert to prove this ordering; replacing the lock with finer-grained locking requires keeping those tests.
- A plan determines whether depleted credits cause blocking or postpaid continuation.
- Reject a free plan that enables postpaid continuation, including a default free plan, because postpaid collection requires a Paddle subscription that free tenants never have. This prevents new invalid plans only; delcore does not migrate or rewrite pre-existing plan rows.
- For plans without postpaid continuation, record already-incurred excess usage as negative credit balance, block subsequent paid use, and offset debt with the next top-up.
- Explicitly document that post-execution reporting cannot prevent all concurrent overspend.
- Delcore owns metering, rating, credits, and settlement. Paddle handles subscription and payment operations through its regular Billing APIs.
- A confirmed renewal payment failure blocks new paid use immediately. Tenant management and payment recovery remain available.
- Paid authorization ends at the confirmed paid period end. A tenant on a paid plan, or with a provider subscription such as a recurring add-on on the free base plan, whose recorded period end is at or before the request time receives the renewal block reason, even when no payment failure is recorded, until a matching confirmed renewal advances the period. There is no grace window: a tenant can be briefly blocked between the period end and delivery of the renewal notification. The check uses the request clock, changes no state, and makes no provider request. Tenant reads report the tenant as blocked under the same condition and request clock. Maintenance queues one billing notice per expired paid period with wording that does not claim a payment failure. The notice is keyed by that period, so a renewed period that expires again notifies again even when maintenance never observed the renewal. Maintenance deactivates the notice when a renewal advances the period or a confirmed payment failure replaces it. Whenever maintenance deactivates a billing notice because its condition no longer holds, it also ends that notice's pending or retrying emails without sending them; emails already sent or in flight are unaffected. Non-paid authorization checks, free-plan calendar periods without a subscription, and tenant management are unaffected.
- Refunds are initiated in Paddle, not through a delcore refund-request workflow.
- Synchronize refund and dispute outcomes. Reverse affected top-up value; block new paid use while a dispute is active or the balance is negative. Restore access only after all applicable blocking conditions clear.

**Deletion and retention**

- Tenant deletion stops new paid use and starts final settlement after the configured usage-reporting grace period. Deletion and withdrawal stop subscription intents that have never been attempted. They also stop subscription intents that the provider definitively rejected without creating a subscription, retiring them as done while retaining the rejection category, including for a rejected intent that an operator already moved back to pending, so an operator retry cannot revive them. Previously attempted, incomplete subscription intents must be reconciled before deletion or withdrawal starts.
- Outstanding debt, disputes, or refundable purchased balances block deletion completion.
- Operators refund remaining purchased credit through Paddle. Synchronize and verify the result before completing deletion.
- After settlement and the configured deletion grace period, delete personal information and retain required accounting/audit records in de-identified form.
- User withdrawal removes delcore access, the personal tenant, and organization memberships subject to ownership and settlement constraints.
- Do not delete the shared Logto identity. Prevent re-registration through the same identity while deletion is pending.
- Require operator-configured retention periods for raw usage, accounting records, audit records, and webhook records. Implement retention jobs without breaking unsettled accounting or required replay protection.

**Interfaces and notifications**

- Provide Connect v1 interfaces for:
  - Current user and tenant discovery; tenant, membership, invitation, ownership, and group management.
  - Catalog reads; subscription changes; manual top-ups and automatic top-up settings.
  - Usage, ledger, and billing history.
  - Service authorization checks, usage reporting, and usage corrections.
  - Operator catalog management, tenant blocking, audit inspection, and failed-job inspection/reprocessing.
- Use typed errors, correlation identifiers, bounded pagination, and UUID v7 for delcore entities. Keep external provider identifiers as explicit external references.
- Authenticate and verify inbound provider events before processing. Apply duplicate and stale-event protection.
- Use Cloudflare Email REST for invitations, insufficient-balance notices, automatic top-up failures, and paid-use blocking notices.
- Send English email only. Organization billing notifications go to Owner and Billing users; personal tenant notifications go to the owner.
- Use operator-allowlisted consumer application URLs for invitation acceptance and payment return flows. Delcore provides the API references and tokens; it does not implement those screens.
- Protect invitation acceptance with recipient identity, expiration, revocation, and single-use semantics.

**Reliability and operational controls**

- Persist work before acknowledging accepted operations. Recover pending work after restart.
- Use durable deduplication and transactional state transitions to prevent repeated ledger effects.
- Reconcile ambiguous payment outcomes with the provider before retrying a charge. Do not blindly repeat a payment whose outcome is unknown.
- Handle duplicate and out-of-order provider notifications without regressing current state.
- Retry transient failures within configured bounds. Route exhausted retries, permanent failures, and payment mismatches to operator review.
- Require validated operator settings for retry limits, timeouts, payload bounds, request rates, invitation expiry, usage lateness, retention, deletion grace, and alert thresholds. Reject missing or contradictory settings at startup.
- Provide structured `log/slog` logs, Prometheus metrics, OpenTelemetry traces, health/readiness checks, audit records, and operational recovery APIs.
- Include safe correlation IDs and bounded error categories. Exclude secrets, tokens, sensitive payloads, and unbounded identity labels from telemetry.
- Expose signals for payment mismatches, settlement delay, delivery failures, retry exhaustion, and durable job backlog.
- Support explicit migrations and backup-before-update procedures. Roll back through a compatible image or documented backup recovery; do not claim arbitrary schema downgrade support.
- Feature flags are not applicable because this is a new independent service. Authorization, billing checks, tenant blocking, and recovery controls remain independently enforced.
- Include a threat model and automated security tests. External security certification or audit is not a completion requirement.

**Documentation and repository integration**

- Create `docs/project-delcore.md`, `docs/servers-delcore-api-contract.md`, and `docs/protos-delcore-v1-contract.md` before runtime implementation.
- Update the documentation catalog, stable project identifiers, and relevant ownership/navigation instructions.
- Keep behavior requirements in the owning contracts rather than copying them into AGENTS files.
- Provide a server README, API usage guidance, self-hosting prerequisites, configuration reference, and backup/recovery/support procedures.
- Document provider setup, external consumer URL requirements, API failure behavior, and the distinction between automated fixtures and live account acceptance.
- Preserve existing service behavior, root development commands, CI affected selection, platform gates, third-party licenses, and validation records.
- Follow repository dependency-security requirements and report unresolved advisories accurately.

### Approved accounting choices

Prorated included quantities are floored to six decimal places (one millionth of a unit). This quantity rule does not round intermediate monetary ledger amounts.

Purchased monetary credits apply only to metered usage. Paddle collects fixed subscription and add-on fees separately. Paid usage periods follow Paddle subscription periods; free usage periods follow UTC calendar months. Automatic top-up monthly caps use UTC calendar months. Schemas and Go bindings live in root `protos/delcore/v1`. Servers consume local packages from the same Go module; see the [repository boundary contract](repository-boundary-contract.md).

## Storage

PostgreSQL stores authoritative entities, immutable price history, append-only ledger entries, replay protection and durable jobs. Persist intent before external requests. Use transactional constraints and locks for concurrent mutations. Explicit migrations run separately from server startup. A database session lock prevents a second instance.

## Security

Validate issuer, audience, signature, expiry and token purpose. Recheck current authorization on each operation. Bind M2M identities to services. Treat provider payloads as untrusted. Verify webhook signatures before persistence. Hash invitation tokens; never log tokens or email bodies. Protect accounting records from direct updates and deletions except controlled retention after settlement.

## Logging

Use log/slog, safe correlation identifiers, bounded error categories, Prometheus and OpenTelemetry. Never place tenant, user, event or provider identifiers in metric labels.

## Build and Test

- Go unit tests for pricing, prorated allowance changes, decimal rounding, plan ranking, role rules, and configuration validation.
- Connect contract tests for typed errors, pagination, authentication, tenancy, and service-scoped reporting.
- PostgreSQL integration tests for migrations, concurrent mutations, append-only corrections, deduplication, job recovery, and retention.
- Sample-client flows covering personal/organization tenants, invitation lifecycle, ownership transfer, group permissions, and free/paid transitions.
- Monthly billing with multiple service meters and add-ons; frozen old prices; late usage and historical corrections.
- Manual and automatic top-ups, concurrent triggers, monthly caps, failure suspension, negative balances, and debt offsets.
- Mocked Paddle payment, refund, dispute, duplicate, reordered, delayed, and ambiguous-response scenarios.
- Mocked Logto token validation, issuer/audience mismatch, expired credentials, unauthorized M2M access, and operator-role enforcement.
- Mocked Cloudflare delivery failures, retries, permanent failures, and correct notification recipients.
- Process termination between provider requests and local acknowledgment, followed by restart and reconciliation.
- Final settlement, outstanding debt, balance refunds, deletion holds, identity re-registration prevention, and de-identification.
- Telemetry redaction, bounded metric labels, audit attribution, operator recovery authorization, and configured alert signals.
- OCI startup, schema incompatibility, graceful shutdown, migration failure, and documented recovery scenarios.
- Repository formatting, Go vet/tests, local protocol consumption, and affected CI validation. Protocol generation/lint/breaking checks run from the repository root. Preserve existing service and workflow behavior.

## Dependencies and Integrations

Use Connect-Go, pgx, OIDC and existing repository telemetry dependencies. Use Paddle regular Billing APIs and Cloudflare Email REST. External provider account acceptance is separate from mocked automated acceptance.

## Change Triggers

Update the [project index](project-delcore.md) and [protocol contract](protos-delcore-v1-contract.md) for boundary changes. Preserve existing behavior and validation records. Instruction updates cover ownership and procedures only.

## References

- [Repository defaults](repository-defaults.md)
- [Dependency security](repository-dependency-security-contract.md)
- [Paddle one-time billing](https://developer.paddle.com/build/subscriptions/bill-add-one-time-charge/)
- [Cloudflare Email](https://developers.cloudflare.com/email-service/)

### Exclusions

- Product UI, admin UI, public documentation app pages, and separate SDK packages.
- Integration changes to existing Delino services.
- Production infrastructure provisioning, Compose deployment bundles, multi-instance operation, rolling deployment, and multi-region support.
- Live provider account acceptance, external security certification, guaranteed performance/SLO targets, and a fixed release date.
- Paddle beta usage accounting.
- Annual subscriptions, trials, tiered pricing, custom rating formulas, multiple currencies, and currency conversion.
- Nested groups, explicit deny permissions, and tenant-defined roles.
- Pre-execution cost reservation or a guarantee of zero overspend.
- User refund requests and delcore-initiated refund execution.
- Global Logto account deletion, general user-data export, and outbound service webhook delivery.
- Feature-flag rollout and migration of existing service data.

### Consumer deletion synchronization

Registered services with the `authorize` scope can read billing-independent tenant state for 1 to 100 known tenant IDs with `GetTenantStates`. The handler reads only `operator_block`, `deleting_at` and `deleted_at` from `tenants` in one transaction and applies the precedence `DELETED`, `DELETING`, `BLOCKED`, `ACTIVE`; IDs without a row are `UNKNOWN`. It never reads plans, subscriptions, ledger, disputes or entitlements, never writes tenant or accounting state, and never calls a payment provider. It returns one entry per requested ID in request order and no tenant names, user details or billing data. A user token, an unregistered service, a registered service without the `authorize` scope or a token without that scope receives a forbidden result; invalid, expired or missing credentials are unauthenticated; invalid IDs or a batch outside 1 to 100 IDs are rejected as invalid input. Database failures return a typed dependency error with a correlation ID and never a state. The success log records the correlation ID, calling service ID, requested count and per-state counts only.

Operate the interface fix-forward: correct a defect with a new release, and do not roll back to a prior image or reverse migrations. The interface adds no migration, feature flag or deployment unit. Consumers fail closed on errors and treat `UNKNOWN` as neither access nor cleanup permission; only `DELETED` permits automatic cleanup of retained tenant data. Automated checks use disposable PostgreSQL with mocked providers and cover all states, overlapping conditions, billing conditions that must not block, batch bounds, malformed and unknown IDs, authorization failures, dependency failure and correlation metadata.

Registered services with the `authorize` scope can also check final deletion for 1 to 100 known tenant IDs with `CheckTenantLifecycle`. Return only IDs with `deleted_at` set. Pending deletion and unknown IDs do not authorize consumer cleanup. Never return tenant names or personal details through this service interface.

## Environment configuration

The server reads individual, unprefixed environment variables and an optional `.env` in its working directory. Process variables, including explicitly empty values, take precedence over `.env`. Both direct and Air startup use the Go `godotenv` parser, including its variable expansion semantics. The JSON `CONFIG` variable and `-config` flag are removed. The service-local `.env.example` lists every setting. Environment names are the uppercase forms of the former JSON keys, for example `DATABASE_URL`, `IDENTITY_KEY`, `PADDLE_TOKEN` and `TIMEOUT_SECONDS`. `RETURN_URLS` is a JSON array of exact URL strings; numeric values use base-10 integers, and `DEVELOPMENT` uses a Go boolean value. An omitted `DEVELOPMENT` is false. `CATALOG_DIR` is optional and selects the development YAML directory; the default is `servers/delcore-api/catalog` when started from the repository root or `catalog` from the service directory. All required settings and existing policy bounds remain validated at startup; invalid values must not appear in logs or errors.

PostgreSQL integration tests use `TEST_DATABASE_URL`. Service-prefixed aliases are not supported. Standard `OTEL_*` and `NO_COLOR` variables retain their names. The consumer example uses its separate `DELCORE_ACCESS_TOKEN` variable.

## Local development

`servers/scripts/dev-api.mjs` owns the development launcher shared by the Go APIs. Its environment handoff follows the [repository workflow](repository-workflow-contract.md#local-development-databases).

The root [development Compose procedure](repository-workflow-contract.md#local-development-databases) provides a local PostgreSQL instance and explicit migration commands. Root `pnpm dev` applies the resolved Compose database URL after Infisical injection through the shared API launcher. The launcher sets `DATABASE_URL` with higher priority than injected/process variables and the service-local `.env`; it preserves the local file and all other settings. Direct service-local development has no Compose override. Follow the [environment contract](repository-environment-contract.md#local-development-injection) for environment precedence and secret boundaries.

The development command first injects Infisical `dev` variables from `/delcore-api` using the local root project configuration. CLI installation, authentication and project access are required; follow the [environment contract](repository-environment-contract.md#local-development-injection).

Run `pnpm dev` at the repository root for all development servers, or from `servers/delcore-api` for this API only. The script runs [Air](https://github.com/air-verse/air) with `root = "../.."` to watch the repository root while build and run commands execute from the service workspace. Copy the service-local `.env.example` to the ignored `servers/delcore-api/.env`, replace placeholders, and choose policy values. Injected/process variables override this file. Keep credentials outside Git. Use `127.0.0.1:8080` for the local listener. A port conflict fails server startup; no alternate port is selected.

Prepare PostgreSQL and apply the existing explicit migration command before starting. Check the [development catalog](#development-catalog) and restart the API after YAML edits; Air does not watch YAML. An absent, empty, invalid, or database-incompatible catalog stops development startup without partial application. Air does not bypass configuration validation, authentication or schema compatibility checks. Stop with `Ctrl+C`; Air sends an interrupt before terminating the server after five seconds. See the [workflow contract](repository-workflow-contract.md) for watcher scope and generated output.
