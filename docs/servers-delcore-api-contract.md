# Delcore API contract

The Rust runtime belongs to the [server Rust workspace](servers-rust-workspace-contract.md). Root development uses `turbo run dev` with the existing workspace launcher.

## Scope

`servers/delcore-api` owns the independent service, migrations and background jobs. The consumer example and protocol source live in root `examples/` and `protos/`.

## Runtime and Language

Use the independent server Cargo workspace and pinned Rust toolchain. Run one process against PostgreSQL. Root `pnpm dev` includes this service through its workspace script.

Build `delcore-api` with the locked server workspace. The OCI image packages the Rust executable, canonical migrations and server/protocol license notices.

## Users and Operators

Authenticated users, registered M2M consumer services, tenant administrators and dedicated platform operators.

## Interfaces and Contracts

The official API server base URL is `https://delcore.api.delino.io`.

**Project and runtime**

- Use the server Rust toolchain and workspace. Run the API and background settlement, retry, notification, and retention jobs in one server process.
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
- Consumer services must reject new paid execution when delcore authorization or billing status cannot be checked. Consumers retain pending usage reports for delivery and reconciliation for 30 days from first creation, then delete them regardless of processing state. This is the shared consumer policy, including DeliTools, not a service-specific exception.

**Catalog and subscription lifecycle**

- Provide platform operator APIs for services, meters, plans, add-ons, and versioned prices.
- Assign persistent UUID v7 identities to human-readable catalog keys. Service and plan keys are catalog-wide; meter, feature, and add-on keys are service-scoped; price keys are scoped to their owning plan or add-on. A source-file move or an equivalent declaration does not change an identity.
- Keep each meter definition immutable for its key, including its event type, aggregation, value attribute, filters, and group-by fields. A changed definition needs a new meter key. A meter key can be referenced by a feature or a price only within its service.
- Support access, metered-limit, and typed configuration features. Configuration types are boolean, integer, decimal, string, and string list. Validate each plan or add-on value against its feature definition. A metered feature names a meter in the same service. Reject a subscription combination when its base plan and an add-on, or two add-ons, define the same feature.
- Keep active catalog versions separate from tenant selections. Version content includes the plan billing policy, selected price versions and feature values. Normalize decimals and order-independent collections before comparison. Reuse an immutable version for equivalent content, including when the catalog returns from A to B to A; display-name edits do not create billing versions. Operator mutations can select new versions while older tenant selections remain intact. Before a plan or add-on mutation changes a product that tenants use without a selected version, such as a product created before catalog versioning, record the current definition as their selection. Backdate that selection so late usage keeps the recorded policy: a plan selection starts at the earlier of the tenant's current billing period start and its open plan entitlements, and an add-on selection starts at the earliest add-on entitlement that has not ended at the mutation. An add-on entitlement with a future end time still counts as held; its selection ends when the last such entitlement ends, and an entitlement without an end keeps the selection open. The start never precedes an earlier selection of the same product or follows the mutation.
- Select the current plan and add-on versions when a tenant is created or an explicit subscription change is requested. When an explicit change selects a plan or add-on that has no version yet, such as a product created before catalog versioning, record its current definition as a version and pin that version. An explicit change must select exactly the catalog price versions of each selected product's active version; legacy price rows without catalog versions are excluded from both sides of that comparison. A free plan whose active version has no catalog prices is selected with an empty price list; an empty list remains invalid for paid plans and for any product whose active version has catalog prices. A pending paid or scheduled change retains its requested version IDs; only provider-confirmed payment or the scheduled free period activates them. Free refresh applies the selected version immediately, and confirmed cancellation selects the then-current default free version. Routine paid renewal keeps the tenant's selected product versions, recurring payment prices, included-usage grants and features. Usage unit prices follow the initial execution start independently of those selections. Previous selections remain available for historical billing and authorization decisions.
- Reject an add-on price whose service differs from the service that owns the add-on, and reject a price that names an unknown add-on, as invalid arguments. Add-on ownership by a service is immutable, so the check at price creation is sufficient; existing prices are not rewritten.
- Each tenant has one base plan and optional service add-ons, consolidated into tenant-wide monthly billing.
- Support free plans without time-limited trials. New tenants start on the operator-designated default free plan.
- Support USD only, with two decimal places for currency minor units. Fix the currency and minor-unit exponent in code; do not expose deployment settings for either value. Prices and Paddle transactions must use USD. Do not perform currency conversion.
- Support fixed unit price multiplied by measured usage, with included free usage. Rate the sum of nonnegative group aggregates against included usage shared once per tenant meter period, as defined in [rating](#rating-and-ledger-reconciliation). Exclude tiered rates, group-specific prices, and custom formulas.
- Operators explicitly rank base plans for upgrade/downgrade classification.
- Apply upgrades after successful immediate prorated payment. Add-ons follow the same immediate prorated addition rule.
- Apply downgrades, add-on removal, and cancellation at the next renewal.
- Preserve existing products and permissions if a requested upgrade payment fails.
- All customers use the usage unit price effective at the initial execution start, including existing subscriptions. Keep that snapshot across execution retries, late usage delivery and corrections. Recurring payment prices and selected product benefits still follow confirmed subscription changes.
- Included usage expires each month. An upgrade grants the prorated difference for the remaining period.
- After paid cancellation takes effect, move the tenant to the free plan and end paid add-ons. Preserve prior debt and disputes separately.
- Treat a confirmed cancellation as complete for every cancel intent of the tenant, including a terminally failed one, so a stale failed cancel job cannot hold final deletion. Duplicate and stale provider notifications keep their ordering protection and do not complete jobs.
- Retain the latest subscription status and occurrence time by provider subscription ID even when no tenant is associated yet. When the initial subscription payment establishes the association, apply retained state that occurred after that payment in the same transaction: a canceled state moves the tenant to the free plan and ends paid products, and a past-due state blocks paid use as renewal failure. The payment is ordered by its provider billing time, capped at the processing time, so a payment that reconciliation or the immediate provider response confirms late is compared with retained state by when it happened and its entitlements start at that time. The confirmed initial payment becomes the subscription's ordering watermark, so a delayed notification that occurred before it stays stale in either delivery order. Retain an unresolved past-due notification separately from the latest status, so a later active status or a past-due notification delivered after a newer active status cannot erase it; a retained cancellation outranks it. Older retained state never overrides a newer payment, the newest occurrence wins regardless of delivery order, and an active status alone never grants paid products or clears a payment failure. Retention applies to notifications processed after this behavior ships; delcore does not backfill or reconstruct notifications that were already acknowledged without retained state.
- Never activate paid entitlements or credit balances solely from a checkout request or browser return.

**Metering, credits, and settlement**

- Meter definitions match an event type and AND-combined scalar attribute filters. Filters support equality, inequality, membership, numeric comparisons, and existence. Filter operands are typed strings, exact decimal numbers, or booleans; equality and membership require the same scalar type. Group keys include sorted attribute names, scalar types, and canonical values so equivalent flat events select the same group without key collisions.
- For each matched meter and group, compute count, sum, min, max, average, or exact unique count. Retain the exact sum and count for averages; round the exposed average to six decimal places using half-up rounding, with negative ties rounded away from zero. Negative numeric inputs remain valid. Replacing or cancelling an event recomputes affected groups and removes empty groups.
- Check group and distinct-value cardinality against the complete accepted event state, including events awaiting aggregate materialization. Apply the admission decision to the whole event when multiple meters match. [Event ingestion](#event-ingestion-and-asynchronous-metering) owns the accepted wire contract, admission accounting, and processing rules.
- Services report measured usage after execution as events. Delcore calculates charges from the group aggregates using its price contracts.
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
- Explicitly document that post-execution reporting cannot prevent all concurrent overspend. Feature limits follow the same rule; see [feature authorization](#feature-authorization-and-metering-queries).
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
- Request deduplication records, consumer usage-delivery/reconciliation records, platform audit records and operational logs expire 30 days after first creation. Delete expired records regardless of processing state, including unknown executions and undelivered or unconfirmed consumer usage. Do not extend expiry after retries or state changes, and do not add an unresolved-record hold. Accept possible new execution and charging after deduplication expiry and permanent loss of pending consumer usage. Stop delivery and unknown-usage lookup at expiry; abandon usage that cannot be established without estimated billing.
- Delcore removes expired request responses and audit rows during maintenance; request handling also removes an expired matching request key before checking replay. Current authorization still applies. Configure external log collection to delete operational logs 30 days after creation. API-key validity is independent of these retention periods. Tenant deletion finalization and acceptance of a user withdrawal delete the affected [API keys](#api-keys).
- The 30-day consumer policy ends at delcore acceptance. It does not delete accepted accounting entries, retained event identities or unfinished delcore processing work. Request response expiry does not remove payment-job or usage-event identities that independently prevent duplicate accounting effects.
- Require operator-configured retention periods for accepted raw usage, accounting records and webhook records. Implement retention jobs without breaking unsettled accounting or required replay protection. Metering events, their revision history and derived rows follow the raw-usage boundary described in [event ingestion](#event-ingestion-and-asynchronous-metering).

**Interfaces and notifications**

- Provide Connect v1 interfaces for:
  - Current user and tenant discovery; tenant, membership, invitation, ownership, and group management.
  - Catalog reads; subscription changes; manual top-ups and automatic top-up settings.
  - Usage, ledger, and billing history.
  - Service authorization checks, including [feature authorization](#feature-authorization-and-metering-queries), and usage reporting and corrections through event ingestion, event corrections, and event reads.
  - Reusable personal and tenant [API keys](#api-keys): creation, metadata listing, revocation and service-side authorization.
  - Billing-period group aggregate queries and metering lag and failure status.
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
- Feature flags are not applicable because this is a new independent, pre-release service; the [pre-release transition](#pre-release-transition) covers the catalog and event metering change. Authorization, billing checks, tenant blocking, and recovery controls remain independently enforced.
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

Purchased monetary credits apply only to metered usage. Paddle collects fixed subscription and add-on fees separately. Paid usage periods follow Paddle subscription periods; free usage periods follow UTC calendar months. Automatic top-up monthly caps use UTC calendar months. Schemas and Go bindings live in root `protos/delcore/v1`. Rust servers generate bindings from those schemas through the local `delino-protos` server crate; see the [repository boundary contract](repository-boundary-contract.md).

### Development catalog

The default development catalog lives at `servers/delcore-api/catalog`; when the service working directory is `servers/delcore-api`, its relative path is `catalog`. `CATALOG_DIR` overrides it with an absolute path or a path relative to the service working directory. The same environment precedence applies as other settings: an Infisical-injected or process value wins over the service-local `.env`, which wins over the default. The development launchers neither set nor read it. Development startup requires an existing directory containing at least one `.yaml` or `.yml` file; a missing directory, a path that is not a directory, or a directory without YAML files stops startup. Discovery is recursive through nested directories and matches the lowercase `.yaml` and `.yml` suffixes. Each file has exactly one entity mapping with a `kind` and stable `key`; unknown fields, duplicate kind/scope/key identities (including identical copies), malformed or multi-document YAML, invalid types, missing references, and catalog constraints reject startup. File names, directory layout, and traversal order do not define identity or reference resolution. The committed catalog splits its declarations across `.yaml` and `.yml` files and nested subdirectories: a service; count, sum, and unique-count meters with typed filters and group-by; access, metered, and all five configuration feature types; a free default plan; a paid plan with a fixed fee, a usage price, and included usage; and an add-on whose feature no plan grants. Tests load and apply it twice so it stays valid. The [split-file example](#split-file-example) below shows another layout.

The supported kinds are `service`, `meter`, `feature`, `plan`, `addon`, and `price`. Service and plan keys are catalog-wide. Meter, feature, and add-on declarations name a service key, and price declarations name a plan key or an add-on `{service, key}` reference. Feature grants name both service and feature keys. A price names its service and meter keys. Keys match `^[a-z][a-z0-9-]{0,99}$`. References can resolve to declarations in other files of the same set or to retained database entities. Each file is authoritative for the entity it declares. The complete set is validated and applied in one transaction; a failure rolls back every change and stops startup. Omitted entities remain in their last database state; omission never deletes or archives them. Replacing an omitted default free plan is a conflict and requires declaring the previous default in the file set. A later development startup restores declared fields and active definitions changed through operator APIs; an omitted entity keeps any operator change. Stable UUID v7 identities survive file moves and restarts. Identical billing content reuses its prior version, including A→B→A; display names, file paths, and order do not create a new billing version. [Catalog version pinning](#catalog-version-pinning) describes the effect on tenants.

Meter `aggregation` values are `count`, `sum`, `min`, `max`, `avg`, and `unique_count`. The event type, aggregation, value attribute, filters, and group-by list are immutable for a meter key. Filter operators are `equal`, `not_equal`, `in`, `greater_than`, `greater_than_or_equal`, `less_than`, `less_than_or_equal`, and `exists`. Filter values are typed YAML scalars. A quoted value is a string, an unquoted integer or decimal is a number and must be a canonical decimal (exponents and special floats are rejected), and unquoted `true` or `false` is a boolean. Equality, inequality, and membership compare the scalar type and canonical value, so a number or boolean attribute never equals a string operand with the same text, and membership values can mix types. Numeric comparisons take one number, written unquoted or as a quoted canonical decimal. `exists` takes no values and requires the attribute to be present with any scalar type. Filters combine with AND. Group-by and membership order do not affect the semantic version. A feature `type` is `access`, `metered` with a same-service `meter`, or `configuration` with a `value_type` of `boolean`, `integer`, `decimal`, `string`, or `string_list`. Access grants use `enabled: true|false`, metered grants use a nonnegative decimal `limit` string, and configuration grants use `configuration` with the declared boolean, integer, quoted decimal, string, or string-list type. A grant supplies exactly one value.

**Catalog constraints**

The loader and the transaction reject the complete set when any of these constraints fails:

- A meter definition differs from the active definition of its key, or a typed declaration changes an untyped meter that already evaluated events (see the [compatibility rule](#event-ingestion-and-asynchronous-metering)). A service has at most 100 meters.
- A feature declaration changes the kind, configuration type, or meter key of its key. A metered feature names a meter that its service does not have.
- A plan has a negative or duplicate rank, or reuses the rank of an omitted plan. More than one plan is the default free plan, the default is not free, a free plan enables `postpaid`, a declared default is cleared without a replacement, or a new default replaces an omitted default.
- An add-on changes its service, or a plan or add-on grants the same feature twice. An add-on grants a feature of another service. A grant value does not match its feature type.
- A price names both or neither of `plan` and `add_on`, uses a currency other than `USD`, has a negative or invalid `unit_price` or `included` decimal, or uses a `price_kind` other than `usage` or `recurring`. A recurring price lacks `external_price_id`. A usage price names a meter that its service does not have. An add-on price names a service other than the add-on's service.

#### Split-file example

Any directory layout works; this one groups declarations by owner and mixes both suffixes:

```text
catalog/
  services/search-api.yaml
  search-api/
    meters/queries.yml
    meters/storage-bytes.yaml
    features/search-access.yaml
    features/monthly-queries.yml
    features/allowed-regions.yaml
  plans/
    search-free.yaml
    search-free/query-usd.yaml
  addons/
    extra-regions.yml
    extra-regions/monthly.yaml
```

The meter in `search-api/meters/queries.yml` counts successful queries per region, and `search-api/meters/storage-bytes.yaml` keeps the largest reported index size per index:

```yaml
kind: meter
service: search-api
key: queries
event_type: search.query
aggregation: count
filters:
  - attribute: status
    operator: in
    values: ["ok", "partial"]
  - attribute: source
    operator: not_equal
    values: ["internal"]
group_by: [region]
```

```yaml
kind: meter
service: search-api
key: storage-bytes
event_type: search.index.size
aggregation: max
value_attribute: bytes
filters:
  - attribute: bytes
    operator: greater_than_or_equal
    values: ["0"]
group_by: [index]
```

The features in `search-api/features/monthly-queries.yml` and `allowed-regions.yaml` reference the meter and declare a configuration type:

```yaml
kind: feature
service: search-api
key: monthly-queries
name: Monthly queries
type: metered
meter: queries
```

```yaml
kind: feature
service: search-api
key: allowed-regions
name: Allowed regions
type: configuration
value_type: string_list
```

`plans/search-free.yaml` grants features declared in other files, and `plans/search-free/query-usd.yaml` prices the meter for that plan:

```yaml
kind: plan
key: search-free
name: Search Free
rank: 0
free: true
default_free: true
postpaid: false
features:
  - service: search-api
    key: search-access
    enabled: true
  - service: search-api
    key: monthly-queries
    limit: "5000"
  - service: search-api
    key: allowed-regions
    configuration: ["eu"]
```

```yaml
kind: price
key: query-usd
plan: search-free
service: search-api
meter: queries
price_kind: usage
unit_price: "0.001"
included: "1000"
currency: USD
```

The add-on in `addons/extra-regions.yml` and its price in `addons/extra-regions/monthly.yaml` use the scoped add-on reference:

```yaml
kind: addon
service: search-api
key: extra-regions
name: Extra regions
```

```yaml
kind: price
key: monthly
add_on:
  service: search-api
  key: extra-regions
service: search-api
meter: subscription
price_kind: recurring
unit_price: "5"
included: "0"
currency: USD
external_price_id: pri_replace_for_local_paddle
```

Moving `query-usd.yaml` to another directory, renaming it, or splitting the plan's grants into another order changes nothing. Declaring `kind: price` with `key: query-usd` for `plan: search-free` in a second file is a duplicate identity even when both files are identical. The same price key under another plan or add-on is a different identity because price keys are scoped to their owner. This example does not grant `allowed-regions` from the add-on, because a subscription that receives the same feature from its base plan and an add-on is rejected. The example assumes a fresh development database. Against a database that already applied the committed catalog, the omitted `example-free` plan keeps rank 0 and the default, so this set fails until it uses another rank and `default_free: false` or the database is reset.

After explicit migration and a successful schema compatibility check, the server acquires the single-instance lock and applies the YAML set only when `DEVELOPMENT=true`. A second instance fails at the lock before it reads the catalog. Synchronization completes before listening or processing jobs. Root, service-local, and Portless development use the same API startup path. Root development builds once before API startup. Restart `pnpm dev` to rebuild Rust or SQL changes, or apply YAML edits. There is no hot reload. Production startup and `migrate` do not apply YAML, and catalog synchronization never applies schema migrations. Production automatic application and standalone validate, diff, or apply commands are out of scope; non-development catalogs use the operator APIs. Correct an invalid file or constraint and restart; do not edit the catalog tables directly to work around a failed transaction. [Recovery](#catalog-and-metering-operations) covers each failure. Restoring an earlier valid definition selects its old version for future explicit selections. Its usage unit prices also apply to subsequent executions by existing subscribers.

### Catalog version pinning

A version is the immutable semantic content of one catalog entity. Meter and feature versions hold their definitions. A price version holds its service, meter, unit price, included usage, currency, external price ID, and price kind; the price `id` is the version and `stable_id` the scoped price key. A plan version holds its rank, free flag, and postpaid policy; plan and add-on versions both hold the active price versions of their price keys and their feature values. Decimals are normalized and order-independent collections are sorted before comparison. Names, file paths, and declaration order are not version content.

- Reapplying unchanged files, or applying the same content after a move or reorder, selects the existing versions and creates nothing.
- Changing a price, an included quantity, a feature value, or a plan rank, free flag, or postpaid policy creates version B and makes it active. Returning the files to the earlier content makes the existing version A active again; no third version is created for A→B→A.
- A display-name-only edit updates the name and creates no billing version.
- A tenant records its plan and add-on versions in `tenant_catalog_selections`. Existing subscriptions keep the versions they selected, including their recurring payment prices, included-usage grants and feature values, through every renewal. A catalog change never moves those product selections. Usage unit prices are resolved separately from activation history at the initial execution start for all subscribers.
- Tenant creation and an explicit subscription change select the versions that are active when they are requested. A pending paid or scheduled change keeps those version IDs until confirmed payment or the scheduled free period installs them. A confirmed cancellation selects the default free version that is active at that time.
- Meter definitions cannot change in place. To meter differently, declare a new meter key, a new metered feature key if a feature limits it, and point the plan's prices and grants at them. That creates a new plan version for future selections. Tenants on the earlier version keep the earlier meter, feature and product selections; usage prices of those products apply according to their effective service and meter. The earlier meter is never deleted, so matching events still contribute to it and count against its admission bounds.

To restore an earlier catalog definition, return the declarations to their earlier content and restart development. New tenants and explicit subscription changes then select the earlier version again. Tenants that selected the intervening version keep it until they make an explicit subscription change. There is no automatic subscription migration and no API that moves existing selections to another version. Restoring a usage price records a new activation of its existing immutable version, so subsequent executions by every subscriber to its owning product use the restored unit price. Unchanged reconciliation creates no activation.

### Call-time usage prices

Every usage price key has append-only activation history. Operator price mutations and development catalog synchronization record an activation only when its active version changes. An activation uses the database clock in the catalog transaction; rolling back the mutation rolls back its activation. A→B→A therefore has three activations and two semantic versions. Prices become effective when activated, without migrating subscriptions.

Resolve the plan and add-ons held at the initial execution start from historical product selections, with entitlement fallback for legacy add-ons. For each price key of those products, choose its latest activation at or before that start. New usage price keys apply to existing product subscribers too. Consumers report that start as `Event.occurred_at` and retain it across retries and reconciliation. Delcore freezes attribution when accepting the original event; processing delays, delivery retries, corrections and later activations do not change it. Price keys remain scoped to their plan or add-on; prices of an unheld product never grant an entitlement. Match the effective version to the event service, meter and usage kind. If several effective keys price the same meter, select the latest activation, then the later price ID. Use legacy entitlement fallback only when no effective catalog price matches; choose the latest entitlement start, then the later price ID.

Migration 11 starts activation history with each price key's current active version at migration time. It does not reconstruct earlier activation times. Occurrences before that baseline, and legacy unversioned prices, use the original entitlement price. Already accepted attributions remain unchanged. Retain activation history as accounting evidence; do not apply the 30-day consumer-record policy to it.

Included-usage grants and recurring payment prices remain subscription benefits and payment terms. They are not recomputed by a usage unit-price activation. All new and existing subscribers to a product use its effective usage unit prices; this is not a DeliTools-only pricing mode.

### Event ingestion and asynchronous metering

Registered services report measured events with `IngestEvent` and correct them with `CorrectEvent` under the existing `usage:write` scope and service ownership checks. `GetEvent` returns the latest accepted revision to the owning service with either usage scope. Event aggregates are the only rating input; [rating](#rating-and-ledger-reconciliation) owns the charge formula and ledger effects.

The quantity reporting calls `ReportUsage` and `CorrectUsage` are retired. They remain declared and deprecated in the protocol for wire compatibility. A caller without the `usage:write` service scope still receives a forbidden result; every other call returns a failed precondition with `ERROR_REASON_PRECONDITION` and a message that names `IngestEvent` or `CorrectEvent`. They never write usage, ledger, or job rows. The legacy `usage_events` and `usage_replay` tables stay in the schema without new writes, so `ListUsage`, retention, and deletion keep working on any rows a development database still holds; reset development databases instead of migrating them.

**Event model and matching**

- An event is one measured execution: a durable service-scoped event key, the tenant, the event type, the occurrence time, and a flat map of scalar attributes. Attribute values are text, booleans, or canonical decimal numbers. Meters refer to attributes by name; nested objects and lists are not supported.
- A meter matches an event when the event type equals the meter's event type and every filter holds. Filters combine only with AND; OR, NOT nesting, and regular expressions are not supported. A filter on an absent attribute fails, except `exists`. `equal`, `not_equal`, and `in` compare against text values, so a number or boolean attribute never equals a filter value and always satisfies `not_equal`. Numeric comparisons hold only for number attributes.
- One event can match several meters of its service. Every matched meter is validated before any of them receives the event; one invalid match, or one cardinality violation, rejects the whole event. An event that matches no meter is rejected.
- Accepting an event and materializing its aggregates are separate steps. Acceptance validates the event, stores it with its processing work, and returns; aggregation, rating, and feature usage follow asynchronously.

**Acceptance**

- An event names a tenant UUID, the authenticated service, a service-scoped event key of 1 to 200 bytes, an event type of 1 to 100 bytes, an occurrence time, and a flat attribute map. A map has at most 100 attributes; names have 1 to 100 bytes; values are strings of at most 1,024 bytes, booleans, or decimal strings of at most 64 characters. Nested values, lists, binary floats, and exponent notation are rejected. Occurrence times are normalized to microseconds and numbers to canonical decimals before hashing and storage.
- The applicable meters are the catalog meters of the calling service when the event is accepted. The evaluated meter set is stored with the event, and corrections match only that set; a meter added later is not backfilled.
- Catalog compatibility rule: a meter registered without a typed definition, such as a `PutMeter` call that supplies only a service and name, uses the compatibility definition. Its event type equals the meter key, its aggregation is `sum` over the numeric `quantity` attribute, and it has no filters or group-by fields. A former quantity report of N units on meter M is therefore an event of type M with `quantity` N. Once an accepted event evaluated an untyped meter, `PutMeter` and catalog synchronization accept a typed declaration for that key only when it equals the compatibility definition; an untyped meter that no event evaluated can still receive any valid typed definition. Acceptance of an original event share-locks the catalog entity of every untyped meter it evaluates and then reads the definitions again, so it serializes with a typing transition that holds the entity lock: a transition that commits first is evaluated by the event, and a later one waits for the event and treats the meter as in use. Typed meters are not locked during acceptance. `GetCatalog` returns an untyped meter without typed fields.
- When the original event is accepted, Delcore attributes one usage price to every evaluated meter: the effective usage price of the plan or add-on held at the initial execution start (`occurred_at`), using [call-time usage prices](#call-time-usage-prices). Corrections keep this attribution, so later price, plan, or entitlement changes never reprice accepted usage. A meter without an applicable usage price has no attribution; its contributions are aggregated but not rated.
- An event that matches no meter returns `ERROR_REASON_NO_MATCHING_METER`. If any matched meter lacks a group-by or value attribute, or a `sum`, `min`, `max`, or `avg` meter receives a non-numeric value, the whole event returns `ERROR_REASON_INVALID_ATTRIBUTE` and no meter receives it. Negative and zero numbers are valid inputs.
- The service and event key form the durable identity. A repeated event with identical canonical content returns the original event ID and revision 1 without repeated effects, before reporting-window and tenant checks and after retention through retained replay identities. Different content under the same key returns `ERROR_REASON_EVENT_IDENTITY_CONFLICT`. Request keys replay their response only within 30 days of first creation. Event identities remain durable independently of request response expiry.
- A future occurrence is invalid input. `USAGE_LATENESS_SECONDS` must be at least 30 days, and `REPORTING_GRACE_SECONDS` must cover it, so consumer delivery can continue for its full retention window. An occurrence older than `USAGE_LATENESS_SECONDS` returns `ERROR_REASON_REPORTING_WINDOW_CLOSED`. A deleted tenant is forbidden. A deleting tenant accepts only occurrences before deletion started and only until `REPORTING_GRACE_SECONDS` after that point; later requests return `ERROR_REASON_REPORTING_WINDOW_CLOSED`.
- The billing period is fixed at acceptance from the occurrence time: the recorded Paddle renewal period that covers it for paid subscriptions, otherwise the UTC calendar month. The calendar month applies only when the plan selected at the occurrence time is free. Paid usage without a recorded covering period returns `ERROR_REASON_PRECONDITION` with a failed-precondition status and stores nothing; the caller retries after the renewal is recorded.
- One transaction stores the event identity, the first revision, its matched meter contributions, admission accounting, and a durable `metering` job before the response. Success promises durable storage and queued processing only. It does not promise read-after-write visibility of aggregates, rating, or entitlement usage; `GetEvent.applied_revision` reports processing progress.

**Corrections**

- A correction names the tenant, event key, expected revision, and either `EVENT_CORRECTION_KIND_REPLACE` with a complete event type and attribute map or `EVENT_CORRECTION_KIND_CANCEL` with no content. Tenant, service, occurrence time, billing period, and the evaluated meter set never change.
- The expected revision must equal the latest accepted revision. Otherwise the request returns `ERROR_REASON_STALE_REVISION` with an aborted status and the caller must re-read the event. A retry whose content equals the already accepted next revision returns that revision without effects. Concurrent corrections of one revision therefore produce one winner.
- A cancelled event cannot be corrected again and returns `ERROR_REASON_EVENT_CANCELLED`. The accepted-lateness bound applies to the original occurrence, and the tenant deletion rules above apply unchanged.
- A replacement is validated like a new event, including admission bounds and group removal or introduction. The original and every correction remain as append-only revisions; database triggers reject updates and deletions outside controlled retention.

**Cardinality admission**

- `MAX_METER_GROUPS` bounds the groups of one tenant, meter, and billing period. `MAX_METER_GROUP_VALUES` bounds the distinct values of one group for `min`, `max`, and `unique_count`, which retain an exact value multiset. `count`, `sum`, and `avg` retain only exact counts and sums and do not consume distinct-value capacity. Unique counts are never approximated.
- Admission accounting follows the latest accepted revision of every event, including revisions that processing has not materialized. A correction releases the contributions of its previous revision before claiming the replacement, so moving an event between groups needs capacity only for the destination. An identity that both revisions share keeps its admission rows and counts unchanged and is not checked as new: this is the group of a meter and, for `min`, `max`, and `unique_count`, also the typed value (kind and canonical text) within that group. Therefore a replacement that retains its group and value succeeds even when it holds the last reference and an operator has lowered a bound below current usage. Only a genuinely new group or distinct value is checked against the state after the release, and a retained group does not exempt a new value from the distinct-value bound. Requests lock the affected period rows in a fixed order, and every state transition already holds the single-instance transaction lock, so concurrent requests and worker backlog cannot exceed the bounds.
- An event that would exceed either bound on any matched meter returns `ERROR_REASON_CARDINALITY_EXCEEDED` with a resource-exhausted status and leaves no partial effect. Materialized aggregates can briefly retain a superseded group until its correction is processed.

**Asynchronous processing**

- Each worker step processes up to 100 ready `metering` jobs before other durable jobs. One transaction claims a job, moves the event's materialized state from its applied revision to its latest revision, rates every changed meter period and appends its ledger difference, runs the optional in-process test hook, and completes the job. A crash before commit leaves the job pending with no aggregate effect; a commit records the applied revision, so repeated or redelivered work changes nothing. Several queued revisions collapse into one exact transition.
- Group aggregates keep the exact event count, exact sum for numeric aggregations, and the distinct-value multiset for `min`, `max`, and `unique_count`. The exposed value is the count, sum, minimum, maximum, distinct count, or the exact average rounded half-up to six places with negative ties away from zero. Removing the last contribution deletes a group; a replacement can remove one group and introduce another. Rating and the hook receive exact before and after aggregates for every changed group.
- A failed attempt rolls back and is recorded separately. Transient failures return to pending after `RETRY_SECONDS` until `RETRY_LIMIT` attempts, then fail with `retry_exhausted`. Inconsistent stored state, including contributions that disagree with materialized aggregates during rating, fails with `metering_invalid`, and an unreadable job payload with `invalid_payload`; neither retries automatically. A shutdown that interrupts an attempt keeps the job pending without consuming an attempt.
- Operators inspect failed work with `ListJobs` and reprocess it with `RetryJob`, which requires a reason, writes the `job.retry` audit record, and returns metering work to pending with a reset attempt count. Pending and failed metering jobs hold final tenant deletion and are never pruned. Tenant billing history omits metering jobs; `GetMeteringStatus` reports their lag and failure state without job details.
- While work is delayed or failed, aggregates and later authorization decisions use the last applied state. Processing lag does not block ingestion or feature authorization.

**Retention and recovery**

- Event identities, revisions, raw attributes, contributions, price attributions, aggregates, rated periods, and admission rows follow the raw-usage boundary. Retention never touches a tenant before its deletion is final, so active and deleting tenants keep everything that pending work and allowed corrections need; final deletion already requires settled accounting and no unfinished metering work. After a final deletion is older than `RAW_USAGE_RETENTION_SECONDS`, maintenance clears the raw attribute maps of the tenant's event revisions; no remaining operation reads them. Identities, fingerprints, contributions, and price attributions remain accounting evidence. After the final deletion is also older than the accounting retention cutoff, maintenance copies replay identities into a separate table and removes those rows in one transaction. Both steps skip a tenant with unfinished metering work. Database triggers permit only these controlled changes.
- Recover a growing backlog by checking worker health, database availability, and `delcore_metering_lag_seconds`; accepted work remains durable while processing is stopped. Recover failed work by reading its category with `ListJobs`, correcting the cause, and calling `RetryJob`. A cardinality rejection is final for that content in that billing period; raise the operator bound and restart, or declare a new meter key with different grouping for future events. Rejected events leave nothing to clean up.
- Logs record the correlation ID, calling service ID, correction kind, revision, replay flag, matched meter count, and typed rejection reason. Worker logs record job IDs, failure categories, attempt counts, and the number of appended rating differences. Logs never contain attribute values, event keys, tenant IDs, or credentials. Prometheus exposes `delcore_metering_backlog`, `delcore_metering_failed_jobs`, and `delcore_metering_lag_seconds` without labels. The [logging](#logging) section lists the ingestion, correction, processing, recovery, and catalog counters, spans, and audit records.

### Rating and ledger reconciliation

Delcore rates one tenant, meter, and billing period at a time from the materialized group aggregates of the last applied revisions. Pending or failed work is not rated until it is applied. Usage prices keep a fixed unit price and free included usage; there are no tiers and no group-specific prices.

**Formula**

- For one usage price and one allowance grant the charge is `max(0, sum(max(0, group_aggregate)) - shared_included_usage) * unit_price`. Each group aggregate is clamped at zero before summing, so a negative group never offsets a positive group. Included usage is shared once by all groups of the tenant meter period and is never granted per group. Groups -5 and 20 with 10 included units bill 10 units.
- The group aggregate is the exposed value: the count, sum, minimum, maximum, average rounded half-up to six places, or exact distinct count. The same rounded average is used for queries, limits, and rating.
- Included usage is the sum of the period's allowance grants for the service and meter key: the renewal or calendar-month grant and prorated upgrade or add-on differences.
- Maintenance issues the free calendar-month grant only for a tenant whose expired free period it advances in that same transaction, so the new period bounds and their grants commit together and a failed renewal retries without missing or duplicating a grant. Tenant creation and confirmed free activation grant their own initial period. Maintenance never re-derives grants for an already initialized period: a service and meter added by an explicit free refresh keeps only its prorated difference grant until the next calendar month, and restarts or repeated maintenance passes add nothing.
- The charge stays exact. Maintenance rounds the cumulative charge of each closed rated period, and of each period of a tenant past its deletion reporting grace, half-up to USD minor units and appends the rounding difference. Individual events and groups are never rounded.

**Price history and mixed-price periods**

- Each contribution carries the usage price attributed when its event was accepted (see [acceptance](#event-ingestion-and-asynchronous-metering)) and an allowance epoch: the number of the period's grants applied at or before its occurrence time. Contributions that share a price and an epoch form a segment. Segments are ordered by epoch, then by their earliest occurrence, then by price ID.
- `count` and `sum` meters split each group's clamped total across the group's segments in that order. Earlier positive contributions are filled first, so a later negative contribution reduces earlier usage of the same group and never moves a charge to another price.
- `min`, `max`, `avg`, and `unique_count` aggregates cannot be decomposed by event. The whole clamped group value belongs to the segment of the group's latest contribution by occurrence time, then by later epoch and price ID.
- Segments consume included usage in order. A segment can use only grants applied at or before the occurrences of its epoch, minus what earlier segments consumed. A grant applied at the same instant as an occurrence is available to it. Each segment's billable remainder is charged at its own unit price. With one price and one grant this is the formula above; it also keeps the earlier rule that usage before an upgrade cannot use the upgrade's prorated allowance.
- Contributions without an attributed price are aggregated but neither consume included usage nor produce a charge.

**Ledger reconciliation**

- In the processing transaction, Delcore rates every meter period whose aggregates changed and compares the exact charge with the amount already charged for that tenant, service, meter, and period by `usage`, `postpaid`, and `correction` entries. Only the difference is appended; existing entries are never rewritten. Every entry keeps the original service, meter key, billing period, and a null `usage_id`.
- An increase from an original event (revision 1) is a `usage` entry; an increase from any later revision is a `correction` entry. On a postpaid period, the part of an increase beyond the tenant's nonnegative credit balance is a `postpaid` entry. Without postpaid continuation the full increase is charged against credits and may make the balance negative, which blocks new paid use until a top-up offsets the debt.
- Every decrease, including replacements, cancellations, groups removed by a replacement, a lower average, maximum, minimum, or distinct count, and clamped negative values, first reverses unpaid `postpaid` charges of the period and returns only the remainder as a positive `correction`, so a correction cannot mint spendable credit.
- The main entry uses source key `rating:<event ID>:<applied revision>:<meter ID>` and the postpaid entry the same key with a `:postpaid` suffix. A rollback leaves no aggregate, rating, or ledger effect; a committed transition is never processed again, so retries, redelivered jobs, operator retries, and replayed ingestion never duplicate ledger effects. Rounding entries use `currency_rounding:` keys.
- A new allowance grant can change the charge of already rated usage without new metering work, for example when a delayed upgrade notification grants a prorated allowance from a provider payment time before rated occurrences. In the transaction that records the grant, Delcore re-rates every rated period of that tenant, service, meter key, and period and appends the difference as a `correction` with source key `allowance:<grant ID>:<meter ID>` (and the `:postpaid` suffix for its postpaid entry). A replayed grant records nothing and appends nothing. A period whose stored metering state cannot be rated keeps its charge and is logged; the failed metering work that owns that state rates the period again when it is recovered.
- Late usage, corrections, and grants keep the original period. A difference in a closed period becomes an adjustment on the next bill: maintenance rounds the new period total again, and settlement charges or credits only the unsettled difference with the original period attribution.
- `metering_rated_periods` records the latest exact charge and remaining included usage of each rated period so rounding can revisit it. The ledger remains the authoritative charged amount.
- Paid authorization treats a non-postpaid tenant with a zero balance as depleted unless a current usage-priced meter of the service has remaining included usage in the tenant's current period. Remaining included usage follows the same rating rule over the last applied aggregates, so accepted but unprocessed events do not consume it yet.

### Group limits and shared included usage

Feature limits and rating read the same applied group aggregates of one tenant, meter, and billing period, but they use them differently:

| Aspect          | Metered feature limit                                      | Usage rating                                                   |
| --------------- | ---------------------------------------------------------- | -------------------------------------------------------------- |
| Scope           | Each group separately                                      | All groups of the tenant meter period together                 |
| Allowance       | The plan or add-on grant's `limit`, applied to every group | Included usage, shared once by all groups                      |
| Negative groups | Below the limit; never offsets another group               | Clamped to zero before summing; never offsets another group    |
| Effect          | Blocks new execution of that group at or above the limit   | Charges `max(0, sum(max(0, group)) - included) * unit_price`   |
| Reset           | At the start of the next billing period                    | Each billing period is rated separately with its own allowance |

Worked example: a `sum` meter grouped by `region` has applied groups `eu` = -5 and `us` = 20 in the current period. The plan grants a metered feature limit of 15 and a usage price with 10 included units. `Authorize` with `region` = `us` returns usage 20 and `BLOCK_REASON_FEATURE_LIMIT_REACHED`; with `region` = `eu` it returns usage -5 and allows execution. Rating clamps `eu` to 0 and bills `max(0, 0 + 20 - 10)` = 10 units. Included usage is not granted per group, so the result is not `max(0, 20 - 10) + max(0, -5 - 10)`, and `eu` does not reduce `us` to 15 billable units.

Billing periods are the recorded Paddle renewal periods for paid subscriptions and UTC calendar months otherwise; there are no daily or weekly entitlement periods. An `avg` group uses the exact average rounded half-up to six decimal places, with negative ties away from zero, and the same rounded value is the limit usage, the query value, and the rating quantity.

### Feature authorization and metering queries

`Authorize` accepts an optional `feature_key` and flat `group_values` in addition to its existing tenant, user, capability, and `paid` fields. An empty feature key keeps the existing permission and billing evaluation and response unchanged and returns no feature result; Git Tracker and other non-paid permission consumers depend on that behavior. Group values without a feature key are invalid input.

**Evaluation order**

- The existing service `authorize` scope and canonical tenant and user IDs are checked first. A feature key must be a stable catalog key. The service-scoped feature definition is read before any tenant data, so malformed requests fail identically for every tenant. Group values for an access or configuration feature, and metered group values that do not name exactly the meter's group-by attributes or are not valid flat scalars, return an invalid-argument error with `ERROR_REASON_INVALID_ATTRIBUTE`. A service can authorize only its own features; another service's key, or a key that the service never declared, returns `BLOCK_REASON_FEATURE_UNKNOWN` without a feature result.
- The unchanged user and service permission check runs next. A permission failure returns only `BLOCK_REASON_PERMISSION` and omits feature details, as before. When `paid` is set, the unchanged billing, renewal, dispute, balance, deletion, and payment-confirmed entitlement checks follow. Feature evaluation then appends its own reason. Feature access never removes another reason, never adds billing checks to a non-paid request (a metered feature without a recorded current paid period only fails closed with the renewal reason, as described below), and does not change `GetTenantStates`, `CheckTenantLifecycle`, or other billing-independent lifecycle state. `allowed` is true only when no reason remains.

**Pinned grants**

- Grants come from `catalog_version_features` of the plan and add-on versions in `tenant_catalog_selections` that are active at the request time. Catalog edits create new versions and never change an existing tenant's selections, so existing subscriptions keep their feature values until an explicit subscription change, confirmed payment, scheduled free transition, or confirmed cancellation selects other versions. Feature definitions (kind, configuration type, and meter key) are immutable for their key.
- No pinned grant, or more than one, returns `BLOCK_REASON_FEATURE_NOT_ENTITLED`. Subscription changes reject duplicate grants, so the second case only fails closed for inconsistent rows and logs a warning.
- An access feature is allowed when its grant enables it; a disabled grant is not entitled. A configuration feature returns its typed boolean, integer, decimal, string, or string-list value and adds no reason. A metered feature returns its canonical decimal per-group limit.

**Metered limits, lag, and overspend**

- The usage of a metered feature is the last applied aggregate of the selected group for the feature's meter in the billing period that contains the request time: the recorded Paddle renewal period for paid subscriptions, otherwise the UTC calendar month. A group without applied events uses zero. For an entitled metered feature, the response returns the limit, usage, and period bounds. Usage resets when a new billing period starts; there are no independent daily or weekly entitlement periods.
- A paid plan whose confirmed period ended before a renewal was recorded has no current billing period, and ingestion rejects usage in it. The limit cannot be measured, so the entitled metered feature returns only its limit, without usage or period bounds, and fails closed with `BLOCK_REASON_RENEWAL`. A paid check that already reported the renewal reason keeps its reasons unchanged; a non-paid request receives only that reason and runs no other billing check. The case is an authorization result, not a dependency error.
- Group values select the group with the same key as ingestion. They must use the scalar types the service reports, because a numeric `7` and the text `"7"` are different groups. Each group has its own limit; included usage of rating is shared per tenant meter and does not change feature limits.
- Execution is blocked with `BLOCK_REASON_FEATURE_LIMIT_REACHED` once the applied aggregate is greater than or equal to the limit, including a zero limit. Averages use the same six-place half-up value as queries and rating. A correction, cancellation, or lower average that brings the applied value below the limit allows the feature again when every other condition permits it.
- Authorization is a pre-execution check followed by post-execution reporting. It never reserves capacity and never counts accepted but unapplied events, and there is no lag-based block or reservation API. Concurrent checks below the limit can all succeed, and execution allowed while processing lags can push the applied value above the limit. Consumers must accept that overspend; the limit then blocks later executions after processing applies it.

**Queries**

- `ListMeterAggregates` returns applied group aggregates for one tenant, one meter named by its service ID and stable key, and the billing period that contains `period_at` (default: the request time). The response names the meter aggregation; a meter without a typed definition reports `sum`, the aggregation of its compatibility definition. Each aggregate returns its flat group values, exposed value, exact event count, exact sum, exact distinct count, and last update time. Decimals are canonical strings. Pages hold 1 to 100 aggregates (default 50) in the order of the SHA-256 group-key digest, the bounded aggregate primary key, so every page reads that key in order instead of sorting all groups of the period; `next_after_id` is an opaque cursor for `Page.after_id` that encodes the last returned digest, so its size does not depend on the group key.
- `GetMeteringStatus` reports, from unfinished `metering` jobs, the accepted events whose latest revision is not applied, pending jobs, the acceptance time and age of the oldest pending job, failed jobs, unfinished jobs per last recorded `MeteringFailureCategory`, and the latest failure time and category. The failure time is when the worker recorded the failure; an operator `RetryJob` request does not change it. Its job categories map `metering_transient`, `retry_exhausted`, `metering_invalid`, and `invalid_payload` to typed values. It never returns job, event, or tenant identifiers; operators use `ListJobs` and `RetryJob` for recovery.
- Query results come from applied asynchronous state. A successful `IngestEvent` or `CorrectEvent` does not imply that an aggregate, a feature usage value, or a status count already includes it.
- Tenant users need the Owner or Billing role for the tenant. Registered services need the `usage:read` scope: they read only their own meters for a supplied tenant ID, and their status counts only work for events they reported. Platform operators may read the status of one tenant or, with an empty tenant ID, the whole service. Other callers receive a forbidden result. Invalid IDs, keys, periods, page sizes, and cursors are invalid input; an unknown meter is invalid input. A `period_at` that falls outside every recorded Paddle period while the tenant's plan at that instant is paid is an invalid period. Errors carry the typed reason and correlation ID like other RPCs.
- Feature authorization and queries are read-only and write no audit records. Logs record the correlation ID, calling service ID, feature key and kind, entitlement, block reason, meter key, query scope, and result counts; they never contain group or attribute values, tenant, user, or event IDs, or credentials.

### API keys

Delcore provides one reusable API key facility for every registered service. It contains no consumer-service-, AI-, Gateway-, model-, translation- or extraction-specific logic: a key grants capabilities that services declared in the catalog, and a service verifies a key through `AuthorizeApiKey`. The [protocol contract](protos-delcore-v1-contract.md#api-keys) owns the public types. API keys do not replace platform-operator credentials or service reporting credentials, and they never authenticate any Delcore operation; a bearer token that starts with `dlk_` is rejected as unauthenticated everywhere.

**Secret format and storage**

- A key has a UUID v7 ID. Its secret is `dlk_<32 lowercase hexadecimal characters of the key ID>_<43 base64url characters without padding of 32 random bytes>`, generated from a cryptographically secure source.
- Persist only the key ID and the SHA-256 digest of the random part. Compare digests in constant time, and perform an equivalent dummy comparison for unknown IDs so timing does not reveal whether a key exists. Never store, log, trace or return the secret except in the first `CreateApiKey` response.
- The `requests` replay record stores the response with `secret` empty. The live first response adds the secret from memory after the transaction and the stored record is never updated with it. A replay within the 30-day window returns the stored metadata and an empty secret; the same request key with different input fails. Lost secrets need a new key.
- Listing returns metadata for current members' own personal keys or the tenant's service keys, including revoked and expired ones, never secrets or digests. Key rows of a revoked or expired key are retained for 30 days after revocation or expiry and are then deleted by maintenance, together with their grants; tenant deletion finalization and acceptance of a user withdrawal delete them earlier; key validity and metadata do not depend on the 30-day request, audit or log retention.

**Ownership, management and limits**

- `PERSONAL` keys belong to the creating user within one tenant. `TENANT` keys are service keys attributed to the tenant; the issuing user is stored for audit only and a tenant key remains valid when that user leaves the tenant.
- Only user tokens manage keys; service identities receive a forbidden result. A personal key can be created, listed and revoked only by its owner, who must be a current member of the tenant. A tenant key can be created, listed and revoked by a current Owner or Admin; the Billing and Member roles and non-members cannot. Another user's personal key is never visible or revocable, including by Owners and Admins; revoking it reports the key as not found. Revocation requires current membership before it looks the key up, so non-members cannot probe key IDs. Creating a key in a tenant that is deleting is a failed precondition, and in a deleted tenant it is forbidden; listing and revoking remain available while the tenant exists. Replaying a stored key mutation first rechecks the caller's current authority.
- A name has 1 to 100 UTF-8 bytes, is not blank and has no control characters. A key has 1 to 16 grants. Each grant names one existing registered service once, by a canonical UUID v7, and 1 to 32 capabilities of 1 to 100 bytes each without control characters, unique within the grant and each declared in the service's `capabilities` when the key is created. Fixed limits allow at most 100 active (neither revoked nor expired) keys per owner scope: personal keys per user and tenant, and tenant keys per tenant; a request over the limit is a failed precondition. Checks run inside the transaction, and the existing single-instance transaction lock serializes concurrent creations, so the limits cannot be exceeded.
- Grants and expiry are immutable. `expires_at` must be after the request time. Without it the key is valid until revoked. There is no update, renewal or rotation operation. Revocation sets `revoked_at` once; repeating it is idempotent and keeps the first time.
- Creation and revocation write `api_key.create` and `api_key.revoke` audit rows attributed to the acting user and the tenant. The reason holds only the key ID (`key_id=<id>`), never the secret, digest or name. Revoking an already revoked key writes no second row. Authorization decisions are not audited; they are logged and counted.

**`AuthorizeApiKey` evaluation order**

1. The caller must be a registered service identity with the `authorize` scope in both its token and its catalog entry, as for `Authorize`; otherwise the result is forbidden. The service ID comes only from that identity.
2. Parse the secret. A malformed secret, an unknown key ID, a digest mismatch, a revoked key, an expired key or a key of a deleted tenant returns the unauthenticated error (`ERROR_REASON_UNAUTHENTICATED`). The same error is returned for all of them and no attribution is disclosed. Delcore cannot tell a bad secret from a rejected service token in this answer, so consumers whose own service credential is misconfigured see every key as invalid.
3. Find the grant for the calling service. The key must grant the requested capability for that service, and the service must still declare it. Otherwise return `allowed=false` with `BLOCK_REASON_PERMISSION` and the key's attribution. A key never authorizes another service's capabilities.
4. Reuse the `Authorize` code path for the key's tenant and the calling service: the tenant must exist and not be deleted; when `paid` is set, apply the same operator, renewal, dispute, balance, deleting and entitlement checks and reasons; a tenant that is being deleted is blocked with `BLOCK_REASON_DELETING` even when `paid` is not set; evaluate `feature_key` and `group_values` exactly as `Authorize` does. Billing logic is shared, not forked. A permission or billing result appends reasons to `reasons`; `allowed` is true only when none remain.
5. For a `PERSONAL` key, the key's user must also pass the existing permission check for the capability in that tenant at the request time: a current member who holds the capability through the Owner role or a group permission, and who is not withdrawing. The effective permission is the intersection of grants and current permissions, so removal from the tenant, group changes and withdrawal block new requests immediately. Membership removal does not revoke the key; the key authorizes again if the same user regains the permission, and the owner may revoke it at any time. For a `TENANT` key, skip the user membership check entirely; the issuing user's membership never matters.
6. Return `allowed`, `reasons`, the optional `feature`, and `key_id`, `tenant_id`, `owner_kind` and `user_id` for any valid secret, including when `allowed` is false.

Delcore reads no caller-provided tenant in this call, so a consumer must compare a request's own tenant with the returned `tenant_id` and reject a mismatch. As with `Authorize`, a result reserves nothing and cannot prevent concurrent overspend; a revocation or expiry applies to the next call, and an execution already running may finish. Consumers fail closed when this call fails or is unavailable.

**Lifecycle, retention and compatibility**

- Tenant deletion finalization deletes all keys and grants of the tenant in the same transaction that finalizes the tenant. Accepting a user withdrawal deletes the user's personal keys in every tenant in the same transaction, and a withdrawing user already fails the permission check. A tenant key issued by a withdrawn user remains valid, and its issuer attribution follows the de-identification applied to other retained records.
- Existing `Authorize`, catalog, billing, event ingestion and consumer behavior remain unchanged. Requiring `paid=true` for a billable service is the consumer's choice, as with `Authorize`.
- Key operations use the existing request-key replay and typed-error rules. `CreateApiKey` and `RevokeApiKey` require a UUID v7 `request_key`.

## Storage

PostgreSQL stores authoritative entities, immutable price history, append-only ledger entries, replay protection and durable jobs. Migration 6 adds scoped stable catalog identities, immutable catalog versions, typed feature definitions, product and version feature values, and tenant catalog selections. Migration 7 adds metering event identities, append-only revisions and contributions, replay identities, materialized group aggregates with value multisets, admission accounting, and a ready-work index for `metering` jobs. Aggregate and admission rows are keyed by the SHA-256 digest of the canonical group key, because a key of several maximum-size group attributes exceeds the PostgreSQL B-tree entry limit; aggregates also keep the full group key and group values. Migration 8 adds append-only per-event usage price attributions, rated meter periods, and a tenant-period index for rating scans; it keeps the unused legacy quantity tables. Rating loads a period's grants once and derives each contribution's allowance epoch from their ordered applied times instead of reading allowance grants once per contribution. Migration 9 adds a partial index of unfinished `metering` jobs so status reads never scan retained completed work, and a job failure time that `RetryJob` does not change. Migration 10 lets controlled retention clear only the raw attribute map of a finally deleted tenant's event revision. Migration 11 adds append-only price activation history and creation-time indexes for request and audit expiry. Migration 12 adds API keys and grants: key rows hold the ID, tenant, owner kind, owner or issuing user, name, SHA-256 digest of the random secret part, creation, optional expiry and optional revocation times; grant rows hold the key, service and capabilities; indexes serve listing and the active-key limits, and triggers reject every change except the first revocation, so grants, expiry and the digest stay fixed. Key rows are not subject to the 30-day request or audit expiry. Catalog synchronization never applies migrations. Persist intent before external requests. Use transactional constraints and locks for concurrent mutations. Explicit migrations run separately from server startup. A database session lock prevents a second instance.

## Security

Validate issuer, audience, signature, expiry and token purpose. Recheck current authorization on each operation. Bind M2M identities to services. Treat provider payloads as untrusted. Verify webhook signatures before persistence. Hash invitation tokens; never log tokens or email bodies. Protect accounting records from direct updates and deletions except controlled retention after settlement. API key secrets are bearer credentials: show them once, store only their SHA-256 digest, compare in constant time, never log them or the digest, and never put them in replay records, metrics, spans or audit rows. Treat any `dlk_` bearer token on a non-`AuthorizeApiKey` operation as unauthenticated.

## Logging

Use log/slog, safe correlation identifiers, bounded error categories, Prometheus and OpenTelemetry. Never place tenant, user, event or provider identifiers in metric labels.

Catalog reconciliation, event ingestion, corrections, asynchronous metering, and operator recovery use these signals. Every label value comes from a fixed set; an unknown category or job kind becomes `other`. Typed `reason` labels are the lowercase `ErrorReason` name without its prefix, `none` on success, or `other`.

| Signal                                            | Labels                                                                                                                           | Meaning                                                                                                                                               |
| ------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- |
| `delcore_catalog_reconciliations_total`           | `outcome`: `applied`, `unchanged`, `rejected`                                                                                    | Development catalog applications. `applied` changed at least one catalog row.                                                                         |
| `delcore_catalog_reconciliation_duration_seconds` | none                                                                                                                             | Application duration, including rejected applications.                                                                                                |
| `delcore_metering_ingestions_total`               | `outcome`: `accepted`, `replayed`, `rejected`; `reason`                                                                          | `IngestEvent` results. A request-key or event-identity replay is `replayed`.                                                                          |
| `delcore_metering_corrections_total`              | `kind`: `replace`, `cancel`, `invalid`; `outcome`; `reason`                                                                      | `CorrectEvent` results.                                                                                                                               |
| `delcore_metering_cardinality_rejections_total`   | `limit`: `groups`, `distinct_values`                                                                                             | Events rejected by `MAX_METER_GROUPS` or `MAX_METER_GROUP_VALUES`.                                                                                    |
| `delcore_metering_jobs_total`                     | `outcome`: `applied`, `deferred`, `failed`; `category`                                                                           | Metering job attempts. `category` is `none` or the recorded failure category.                                                                         |
| `delcore_metering_processing_duration_seconds`    | `outcome`                                                                                                                        | Duration of one metering job attempt.                                                                                                                 |
| `delcore_operator_job_retries_total`              | `kind`: job kind                                                                                                                 | Failed jobs that an operator returned to processing with `RetryJob`.                                                                                  |
| `delcore_api_key_operations_total`                | `operation`: `create`, `list`, `revoke`, `authorize`; `outcome`: `succeeded`, `allowed`, `denied`, `unauthenticated`, `rejected` | API key results. `denied` is an `AuthorizeApiKey` answer with `allowed=false`; invalid, unknown, revoked and expired secrets share `unauthenticated`. |

API key logs record the correlation ID, calling service ID, key ID, tenant ID, owner kind, capability, outcome and cause. They never contain secrets, digests or key names, and key, tenant and user IDs never become metric labels. OpenTelemetry spans `delcore.catalog.reconcile`, `delcore.event.ingest`, `delcore.event.correct`, and `delcore.metering.process` carry the correlation ID or job ID, the outcome, the typed reason or failure category, and counts such as revision, matched meters, changed groups, files, and created versions. They never carry tenant, user, event, or service IDs, event keys, attribute values, or credentials. An idle worker step creates no metering span. A shutdown that interrupts a claimed metering attempt ends its span with outcome `deferred` and failure category `shutdown`; it records no job metric because the job stays pending without consuming an attempt.

Audit records cover state changes without one row per ingested event:

- `catalog.reconcile`: one record per development application that changed catalog state, attributed to `system:catalog`, without a tenant. The reason lists file and per-kind declaration counts and created entity and version counts. An unchanged or rejected application writes none.
- `event.correct`: one record per new correction, attributed to the calling `service:<id>` with the event's tenant. The reason holds the correction kind, new revision, and matched meter count, never the event key or content. Replays and rejected corrections write none.
- `metering.failed`: one record when metering work fails permanently or exhausts retries, attributed to `system:metering` with the tenant. The reason holds the failure category and attempt count. Transient deferrals write none.
- `job.retry`: the operator `RetryJob` record with the operator reason. Its log records the job ID, job kind, and previous failure category, not the reason.

## Build and Test

- Rust unit tests for pricing, prorated allowance changes, decimal rounding, plan ranking, role rules, and configuration validation.
- Connect contract tests for typed errors, pagination, authentication, tenancy, and service-scoped reporting.
- PostgreSQL integration tests for migrations, concurrent mutations, append-only corrections, deduplication, job recovery, and retention. Cover call-time usage prices for existing and new subscribers, A→B→A and unchanged synchronization, late delivery and corrections, fixed 30-day request/audit boundaries without retry extension, fresh requests after expiry, and preservation of accepted accounting and unfinished metering work.
- Sample-client flows covering personal/organization tenants, invitation lifecycle, ownership transfer, group permissions, and free/paid transitions.
- Monthly billing with multiple service meters and add-ons; frozen old prices; late usage and historical corrections.
- Event rating: clamped groups with shared included usage, increases and decreases from corrections and non-additive aggregates, mixed-price periods with prorated grants, postpaid reversal before credit, late closed-period adjustments with final rounding, rollback and retry idempotency, and the untyped-meter compatibility rule.
- Manual and automatic top-ups, concurrent triggers, monthly caps, failure suspension, negative balances, and debt offsets.
- Mocked Paddle payment, refund, dispute, duplicate, reordered, delayed, and ambiguous-response scenarios.
- Mocked Logto token validation, issuer/audience mismatch, expired credentials, unauthorized M2M access, and operator-role enforcement.
- Mocked Cloudflare delivery failures, retries, permanent failures, and correct notification recipients.
- Process termination between provider requests and local acknowledgment, followed by restart and reconciliation.
- Final settlement, outstanding debt, balance refunds, deletion holds, identity re-registration prevention, and de-identification.
- Telemetry redaction, bounded metric labels, audit attribution, operator recovery authorization, and configured alert signals.
- API keys: both owner kinds, creation by every role, management permissions, service and capability restrictions, tenant isolation, one-time secret and its absence from replay records, listings, logs and audit rows, expiry and revocation, personal-user removal and withdrawal, tenant-key independence from issuer removal, cross-service authorization attempts, malformed and wrong secrets with uniform errors, limits and deletion cleanup, retention independence, and `Authorize` compatibility. Repeat the common flows with a registered service that is not DeliTools, using catalog configuration alone.
- OCI startup, schema incompatibility, graceful shutdown, migration failure, and documented recovery scenarios.
- Repository formatting, server Cargo Clippy/tests, local protocol consumption, and affected CI validation. Protocol generation/lint/breaking checks run from the repository root. Preserve existing service and workflow behavior.

## Dependencies and Integrations

Use Connect RPC, SQLx, OIDC and the server workspace telemetry dependencies. Use Paddle regular Billing APIs and Cloudflare Email REST. External provider account acceptance is separate from mocked automated acceptance.

## Change Triggers

Update the [project index](project-delcore.md) and [protocol contract](protos-delcore-v1-contract.md) for boundary changes. Preserve existing behavior and validation records. Instruction updates cover ownership and procedures only.

## References

- [Repository defaults](repository-defaults.md)
- [Dependency security](repository-dependency-security-contract.md)
- [Paddle one-time billing](https://developer.paddle.com/build/subscriptions/bill-add-one-time-charge/)
- [Cloudflare Email](https://developers.cloudflare.com/email-service/)

### Exclusions

- Product UI, admin UI, public documentation app pages, and separate SDK packages.
- API key editing, renewal and rotation operations, key use against Delcore operations, and replacement of platform-operator or service-reporting credentials with keys.
- Integration changes to existing Delino services.
- Production infrastructure provisioning, Compose deployment bundles, multi-instance operation, rolling deployment, and multi-region support.
- Live provider account acceptance, external security certification, guaranteed performance/SLO targets, and a fixed release date.
- Paddle beta usage accounting.
- Annual subscriptions, trials, tiered pricing, custom rating formulas, multiple currencies, and currency conversion.
- Nested groups, explicit deny permissions, and tenant-defined roles.
- Pre-execution cost reservation or a guarantee of zero overspend.
- User refund requests and delcore-initiated refund execution.
- Global Logto account deletion, general user-data export, and outbound service webhook delivery.
- Feature-flag rollout and migration of existing service data, including production migration of quantity usage to events.
- OpenMeter installation, integration, or API and schema compatibility.
- YAML hot reload, standalone catalog validate, diff, or apply commands, production automatic catalog application, implicit deletion of omitted catalog entities, and automatic schema migration during catalog synchronization.
- In-place meter definition changes, automatic subscription migration to new catalog versions, and retroactive backfill when a meter is added.
- Nested event attributes, OR or NOT filter expressions, regular-expression filters, approximate unique counts, arbitrary time-range aggregate queries, and hourly or daily query buckets.
- Independent daily or weekly entitlement periods, cross-group negative offsets, per-group included-usage grants, and lag-based blocking.

### Consumer deletion synchronization

Registered services with the `authorize` scope can read billing-independent tenant state for 1 to 100 known tenant IDs with `GetTenantStates`. The handler reads only `operator_block`, `deleting_at` and `deleted_at` from `tenants` in one transaction and applies the precedence `DELETED`, `DELETING`, `BLOCKED`, `ACTIVE`; IDs without a row are `UNKNOWN`. It never reads plans, subscriptions, ledger, disputes or entitlements, never writes tenant or accounting state, and never calls a payment provider. It returns one entry per requested ID in request order and no tenant names, user details or billing data. A user token, an unregistered service, a registered service without the `authorize` scope or a token without that scope receives a forbidden result; invalid, expired or missing credentials are unauthenticated; invalid IDs or a batch outside 1 to 100 IDs are rejected as invalid input. Database failures return a typed dependency error with a correlation ID and never a state. The success log records the correlation ID, calling service ID, requested count and per-state counts only.

Operate the interface fix-forward: correct a defect with a new release, and do not roll back to a prior image or reverse migrations. The interface adds no migration, feature flag or deployment unit. Consumers fail closed on errors and treat `UNKNOWN` as neither access nor cleanup permission; only `DELETED` permits automatic cleanup of retained tenant data. Automated checks use disposable PostgreSQL with mocked providers and cover all states, overlapping conditions, billing conditions that must not block, batch bounds, malformed and unknown IDs, authorization failures, dependency failure and correlation metadata.

Registered services with the `authorize` scope can also check final deletion for 1 to 100 known tenant IDs with `CheckTenantLifecycle`. Return only IDs with `deleted_at` set. Pending deletion and unknown IDs do not authorize consumer cleanup. Never return tenant names or personal details through this service interface.

## Environment configuration

The server reads individual, unprefixed environment variables and an optional `.env` in its working directory for every invocation. Process variables, including explicitly empty values, take precedence over `.env`. The Rust loader preserves automatic file loading for development, production and operator commands; the shared launcher also passes the accepted `--dev-env` flag. The JSON `CONFIG` variable and `-config` flag are removed. The service-local `.env.example` lists every setting. Environment names are the uppercase forms of the former JSON keys, for example `DATABASE_URL`, `IDENTITY_KEY`, `PADDLE_TOKEN` and `TIMEOUT_SECONDS`. `RETURN_URLS` is a JSON array of exact URL strings; numeric values use base-10 integers, and `DEVELOPMENT` accepts `true`, `false`, `1`, `0`, `t`, `f`, `T`, `F`, `TRUE`, `FALSE`, `True` or `False`. An omitted `DEVELOPMENT` is false. `MAX_METER_GROUPS` and `MAX_METER_GROUP_VALUES` are required base-10 integers from 1 to 1,000,000 and set the [cardinality admission](#event-ingestion-and-asynchronous-metering) bounds. `CATALOG_DIR` is optional and selects the development YAML directory; the default is `servers/delcore-api/catalog` when started from the repository root or `catalog` from the service directory. All required settings and existing policy bounds remain validated at startup; invalid values must not appear in logs or errors.

PostgreSQL integration tests use `TEST_DATABASE_URL`. Service-prefixed aliases are not supported. Standard `OTEL_*` and `NO_COLOR` variables retain their names. The consumer example uses its separate `DELCORE_ACCESS_TOKEN` user token for tenant discovery and `DELCORE_SERVICE_TOKEN` service M2M token for the event metering flow.

## Local development

`servers/scripts/dev-api.mjs` owns the development launcher shared by the Rust APIs. Its environment handoff follows the [repository workflow](repository-workflow-contract.md#local-development-databases).

The root [development Compose procedure](repository-workflow-contract.md#local-development-databases) provides a local PostgreSQL instance and explicit migration commands. Root `pnpm dev` applies the resolved Compose database URL after Infisical injection through the shared API launcher. The launcher sets `DATABASE_URL` with higher priority than injected/process variables and the service-local `.env`; it preserves the local file and all other settings. Direct service-local development has no Compose override. Follow the [environment contract](repository-environment-contract.md#local-development-injection) for environment precedence and secret boundaries.

The development command first injects Infisical `dev` variables from `/delcore-api` using the local root project configuration. CLI installation, authentication and project access are required; follow the [environment contract](repository-environment-contract.md#local-development-injection).

Run `pnpm dev` at the repository root for all development servers, or from `servers/delcore-api` for this API only. Root Turbo development builds the server workspace through `//#server-build`; the launcher executes the compiled `delcore-api` binary directly. Before a service-local launch, run `cargo build --locked --manifest-path servers/Cargo.toml --workspace --bins` from the repository root. Install `protoc` before compiling. Copy the service-local `.env.example` to the ignored `servers/delcore-api/.env`, replace placeholders, and choose policy values. Injected/process variables override this file. Keep credentials outside Git. Use `127.0.0.1:8080` for the local listener. A port conflict fails server startup; no alternate port is selected.

Prepare PostgreSQL and apply the existing explicit migration command before starting. Check the [development catalog](#development-catalog) and restart the API after YAML edits; development does not automatically restart the API. An absent, empty, invalid, or database-incompatible catalog stops development startup without partial application. Development startup retains configuration validation, authentication and schema compatibility checks. Stop with `Ctrl+C`; Turbo owns shutdown. See the [workflow contract](repository-workflow-contract.md) for build, restart and generated-output behavior.

## Catalog and metering operations

### Pre-release transition

Delcore has no deployed data that must be preserved. The YAML catalog, catalog versions, feature entitlements, event ingestion, aggregate rating, and feature authorization ship as one coordinated pre-release change: the protocol, server, repository callers, fixtures, examples, and contracts change together in the same review. No feature flag or staged rollout gates them. The deprecated `ReportUsage` and `CorrectUsage` calls return a failed precondition instead of billing, so a caller that still reports quantities must move to `IngestEvent` and `CorrectEvent` with the server.

There is no production data migration. Migrations do not convert legacy quantity usage into events, and legacy `usage_events` rows are retained but never rated. Reset a development database instead of migrating its usage or catalog data. Migrations stay explicit through `migrate`, transactional, and separate from catalog synchronization: normal startup only checks schema compatibility, and `migrate` never reads the catalog.

To reset the local Compose database, stop the Delcore API and run from the repository root:

```sh
docker compose exec delcore-postgres dropdb -U delcore --force delcore
docker compose exec delcore-postgres createdb -U delcore delcore
(cd "servers/delcore-api" && ../target/debug/delcore-api migrate --dev-env)
```

The next development startup applies the catalog again. A reset database assigns new UUID v7 identities to every catalog key, so discard any Delcore IDs that local consumers stored from the earlier database. `docker compose down --volumes` also resets the DevBase and Git Tracker databases.

### Recovery procedures

These procedures use the existing settings, recovery APIs, and backup rules. They do not define performance or recovery-time targets.

- **Invalid YAML or catalog files.** Development startup exits before listening and reports the catalog error with the affected file path; a duplicate identity names both files. Causes include a missing or empty directory, malformed or multi-document YAML, an unknown kind or field, an invalid key or value, a missing reference, and a duplicate identity. Files are validated before the transaction starts, and the transaction rolls back on any later error, so the database keeps its previous catalog. Correct the files, or point `CATALOG_DIR` at the intended directory, and restart the API task. YAML-only edits do not restart the API.
- **Catalog constraint failures.** A [catalog constraint](#development-catalog) failure rolls back the whole set and stops startup in the same way. Change the declaration instead of the database: declare a new meter or feature key instead of changing a definition, choose an unused plan rank, or declare the omitted default free plan with `default_free: false` next to its replacement. Do not edit catalog tables directly. When a development database holds state that is no longer needed, reset it as described above.
- **Database failure.** Startup stops when PostgreSQL is unreachable, the schema is missing or has another version, or another instance holds the instance lock. Restore database service, run `migrate` from the matching image or revision, or stop the other instance, then restart. During operation, requests that need the database fail with a typed error and acknowledge nothing. Reporting services keep the event and resend it with the same event key; identical content never applies twice. A failed worker attempt rolls back and retries after `RETRY_SECONDS` until `RETRY_LIMIT` attempts. The process stops when it loses the instance-lock connection; restart it after the database recovers.
- **Asynchronous backlog.** A growing `lag_seconds` or pending count in `GetMeteringStatus`, the metering backlog and lag metrics, or a `GetEvent.applied_revision` below the latest revision shows that processing is behind. Accepted events stay durable. Aggregates, feature usage, and rating keep the last applied state, and authorization keeps using it, so lag can allow overspend but never blocks ingestion or authorization. Check that the process is running and ready and that PostgreSQL is healthy. The worker drains pending work when it runs; do not delete or edit jobs.
- **Exhausted retries and permanent failures.** `GetMeteringStatus` reports failed jobs by `MeteringFailureCategory`. Operators list failed `metering` jobs with `ListJobs` and read the `retry_exhausted`, `metering_invalid`, or `invalid_payload` category. Fix the cause first: database availability or capacity for exhausted retries, and inconsistent stored state or an unreadable payload for the other two, which need a code fix or a development database reset. Then call `RetryJob` with a reason; it writes the `job.retry` audit record and returns the job to pending with a reset attempt count. Pending and failed metering work is never pruned and holds final tenant deletion.
- **Cardinality rejection.** `ERROR_REASON_CARDINALITY_EXCEEDED` stores nothing, so the reporting service keeps the event. `MAX_METER_GROUPS` bounds the groups of one tenant, meter, and billing period. `MAX_METER_GROUP_VALUES` bounds the distinct values of one group for `min`, `max`, and `unique_count` meters. Both accept 1 to 1,000,000 and are read at startup, so change the setting and restart the server. Raising a bound permits more admission, aggregate, and value rows for each tenant meter period, which increases storage, per-event processing, rating work over all groups of the period, and aggregate query pages. Lowering a bound rejects only new groups or new distinct values; existing ones remain and keep accepting events. Each billing period starts with empty admission state, and corrections or cancellations that remove a group or value release its capacity, while a correction that retains a group or value keeps it even when it holds the last reference. When capacity is available, resend the rejected event with the same key and content. To group future events differently, declare a new meter key and select it through a new catalog version. Choose bounds from expected tenant cardinality and database capacity.
- **Restoring a previous catalog definition.** Return the declarations to their earlier content, for example by checking out the earlier catalog files, and restart development. Version reuse makes the earlier version active again for new tenants and explicit subscription changes. Existing subscriptions keep their selected product benefits and recurring payment prices. Their subsequent usage executions use the restored effective unit prices of their held products. Removing a file does not restore anything, because omitted entities keep their database state; declare the entity with its earlier content instead.
- **Compatible image or backup recovery.** Back up and verify the database before running `migrate`, as the [server README](../servers/delcore-api/README.md#operations-and-recovery) describes. An earlier image starts only when it accepts the installed schema version. Otherwise restore the verified backup into a separate database with matching configuration and run a compatible image. There is no arbitrary schema downgrade. Pending metering jobs in the backup resume after restart. Events accepted after the backup are absent; reporting services that still hold them can resend them with the same keys within the accepted lateness.
