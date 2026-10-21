# Delcore API

Delcore owns shared personal and organization tenants, service authorization, usage accounting and tenant-wide billing. It runs a Connect API and durable background jobs in one Rust process with PostgreSQL. It has no UI or separate SDK package.

The [project index](../../docs/project-delcore.md), [server contract](../../docs/servers-delcore-api-contract.md) and [protocol contract](../../docs/protos-delcore-v1-contract.md) define the full behavior contract. Repository-owned files in `servers/` use [FSL-1.1-ALv2](../LICENSE).

## Operator prerequisites

- A PostgreSQL database dedicated to this service, with a separate backup destination. Use a migration role that can create schema objects. Restrict application access to the owning database.
- An operator-owned Logto issuer, API audience, dedicated operator role, and distinct M2M applications for each service. User access tokens must include a verified email claim. Never use an ID token as an API access token. Register each M2M client and its allowed scopes through the operator API.
- A Paddle Billing account with monthly recurring prices, an appropriate product for purchased usage credits, an API token, and a signed notification destination at `/webhooks/paddle`. Subscribe to transaction completion/failure, subscription lifecycle and adjustment creation/update events. Use sandbox credentials for validation.
- Cloudflare Email Sending enabled for the sender domain, an account-scoped sending token, and valid sender DNS configuration. Email Sending remains beta. A Workers Paid plan is required for arbitrary recipients; consult the [current setup guide](https://developers.cloudflare.com/email-service/get-started/). The server uses REST, not a Workers binding.
- Consumer application pages for invitation acceptance and payment return. Register their exact URLs. Delcore does not supply these screens.
- A TLS reverse proxy for public API access. Expose health and metrics endpoints only to trusted monitoring infrastructure. Do not publish PostgreSQL.

## Configuration and startup

Copy `.env.example` to an uncommitted `.env` in the service working directory, replace every placeholder, restrict file permissions and choose policy limits for your deployment. The example retention periods are examples, not a statement of legal requirements. Keep the identity key stable across updates: it derives the stored identity reference. Loss or unplanned rotation prevents identity matching.

```sh
../target/debug/delcore-api migrate --dev-env
../target/debug/delcore-api serve --dev-env
```

First build from the repository root with `cargo build --locked --manifest-path servers/Cargo.toml -p delcore-api` (`protoc` is required). Run these commands from `servers/delcore-api`. Every invocation loads the optional working-directory `.env`; process variables, including explicitly empty values, take precedence. The shared development launcher passes the accepted `--dev-env` flag. Migrations are explicit and transactional. Normal startup checks schema compatibility; it does not apply migrations. A database session lock rejects a second running instance. The service stops if it loses that lock connection.

With `DEVELOPMENT=true`, normal startup reads the [split-file YAML catalog](catalog) after the schema check and instance lock, then reconciles declared entities in one transaction before serving requests or processing jobs. The default is `catalog` from this working directory. Set `CATALOG_DIR` to another existing directory to use your own declarations. It scans nested `.yaml` and `.yml` files, with one entity per file; absent, empty, malformed, or inconsistent catalogs stop startup without partial changes. The examples declare a service, meter, features, plan, add-on, and prices in separate files. Meters and features use service keys; add-on prices name both the owning service and add-on key. See the [development catalog contract](../../docs/servers-delcore-api-contract.md#development-catalog) for the full schema, literal semantics, reference rules, constraints, and a [nested split-file example](../../docs/servers-delcore-api-contract.md#split-file-example).

Files restore declared fields changed through operator APIs. Omitted entities remain in the database. Unchanged content reuses stable IDs and versions, and returning from definition A to B to A reselects A for future subscriptions without moving existing subscriptions. Meter definitions cannot change in place; declare a new meter key instead. See [catalog version pinning](../../docs/servers-delcore-api-contract.md#catalog-version-pinning). To recover from invalid YAML or a catalog constraint, correct the files or conflicting declaration and restart. If PostgreSQL is unavailable, restore database service first. YAML edits alone do not restart the API; restart the API task. Production startup and `migrate` do not read the catalog. Keep real credentials out of catalog files.

Outside development, bootstrap the service catalog and default free plan through the operator APIs with a user token carrying the dedicated operator role; production startup has no automatic catalog application and there are no standalone catalog commands. Create registered meters before creating usage prices. Register all prices before marking the free plan as the default. Price versions are immutable. Existing entitlements refer to their chosen price version. Keep fixed recurring prices separate from usage prices.

| Settings                                                      | Purpose                                                |
| ------------------------------------------------------------- | ------------------------------------------------------ |
| `LISTEN`, `DATABASE_URL`                                      | HTTP listener and authoritative PostgreSQL connection  |
| `ISSUER`, `AUDIENCE`, `OPERATOR_ROLE`, `IDENTITY_KEY`         | Authentication and stable identity matching            |
| `PADDLE_*`                                                    | Billing API, credit product and webhook authentication |
| `EMAIL_*`                                                     | Cloudflare REST account, sender and authentication     |
| `RETURN_URLS`                                                 | Exact consumer URL allowlist                           |
| `TIMEOUT_SECONDS`, `MAX_PAYLOAD_BYTES`, `REQUESTS_PER_MINUTE` | Bounded HTTP and provider work                         |
| `RETRY_LIMIT`, `RETRY_SECONDS`                                | Durable retry limits and delay                         |
| `INVITATION_SECONDS`, `USAGE_LATENESS_SECONDS`                | Invitation expiry and accepted reporting lateness      |
| `REPORTING_GRACE_SECONDS`, `DELETION_GRACE_SECONDS`           | Final reporting and deletion delays                    |
| `*_RETENTION_SECONDS`                                         | Explicit retention policy inputs                       |
| `BACKLOG_ALERT`, `SETTLEMENT_ALERT_SECONDS`                   | Operational alert thresholds                           |
| `MAX_METER_GROUPS`, `MAX_METER_GROUP_VALUES`                  | Event admission bounds per tenant meter period         |
| `DEVELOPMENT`                                                 | Permit loopback HTTP provider URLs for local fixtures  |
| `CATALOG_DIR`                                                 | Optional development YAML catalog directory            |

All billing uses USD with two decimal places for currency minor units. Currency and minor-unit precision are fixed and have no configuration settings.

The server reads individual environment variables and the optional working-directory `.env`, with process variables taking priority, including explicitly empty values. `RETURN_URLS` is a JSON array of exact URL strings. Integer settings use base-10 values. `DEVELOPMENT` accepts boolean values and defaults to false. The loader rejects invalid values, missing required settings and contradictory policies. The `CONFIG` variable and `-config` flag are no longer supported. Optional standard OTLP environment variables configure tracing.

## Consumer API

Generate clients from the `delcore.v1` schemas in [Delino Apps](https://github.com/delinoio/apps) or use the generated Go Connect client. Every mutation supplies a UUID v7 `request_key`. Repeating a key with different input returns a conflict. Responses and typed errors carry a correlation ID.

The sample client reads a token from the environment and prints only tenant count and correlation metadata. From the repository root:

```sh
go run ./examples/delcore-client -endpoint "http://127.0.0.1:8080"
```

User clients discover their personal tenant, create organizations, manage invitations and groups, and inspect billing through their current roles. Owner transfer requires acceptance by the receiving member. Admin has no implicit billing authority and cannot grant privileged roles.

M2M clients call `Authorize`, the event calls below and service-scoped `ListUsage` under their registered service identity. Treat an unavailable authorization decision as a denial of new paid execution. Retain pending usage events locally until accepted. Use a stable event key for every measured execution. Corrections and ledger effects append new records. The deprecated `ReportUsage` and `CorrectUsage` calls no longer bill and return a failed precondition. A meter declared without typed fields accepts the former quantity report as an event whose type is the meter key and whose numeric `quantity` attribute is the quantity.

Event-based callers use `IngestEvent`, `CorrectEvent` and `GetEvent`. Send each measured execution once with a stable event key, its event type, occurrence time and flat attributes; repeat it unchanged after a timeout. A success means Delcore stored the event and queued aggregation, not that aggregates are already updated. A correction names the latest revision and replaces the complete type and attributes or cancels the event; after `ERROR_REASON_STALE_REVISION`, read the event again before deciding. Events that match no meter, miss a required attribute, or exceed the configured group or distinct-value bound are rejected as a whole with a typed reason. See the [event ingestion contract](../../docs/servers-delcore-api-contract.md#event-ingestion-and-asynchronous-metering).

To check a catalog feature before execution, add its `feature_key` to `Authorize`; for a metered feature, also send `group_values` that name exactly the meter's group-by attributes with the scalar types you report. The response keeps every permission and billing reason and adds a feature result: access state, the typed configuration value, or the per-group limit with the last applied usage of the current billing period. Without a feature key the response is unchanged. Limits block only after processing has applied usage that reaches them. Authorization reserves nothing, so concurrent or lagging executions can exceed a limit; report every execution anyway. Services with `usage:read` read their own applied aggregates with `ListMeterAggregates` and processing lag and failures with `GetMeteringStatus`. See [feature authorization](../../docs/servers-delcore-api-contract.md#feature-authorization-and-metering-queries).

Post-execution reporting cannot guarantee zero overspend under concurrency. A non-postpaid tenant may incur negative credits; the next top-up offsets that debt. Monetary values are decimal strings. Prorated included quantities are floored to 0.000001; monetary products keep full precision until cumulative service/meter period totals are rounded half-up to currency minor units. Occurrence timestamps are normalized to PostgreSQL microsecond precision. Credits pay usage only. Fixed subscription and add-on fees remain Paddle charges. Negative postpaid corrections carry forward against later usage. On cancellation, refund any remaining usage overpayment through Paddle. Select the original usage transaction item; an approved refund clears the settlement hold after webhook processing. A fixed subscription fee refund does not clear usage debt. Keep the transaction item attribution when restoring a backup; an unverified refund must remain under operator review.

Payment requests return a durable job ID. Poll `ListBillingHistory` for payment state and checkout URLs. A checkout return is not proof of payment. One-time charges use Paddle price metadata to correlate the resulting transaction; the subscription response itself never grants credits. Recover payment failures through the payment provider and inspect independently enforced blocking reasons.

## Operations and recovery

Use `/healthz`, `/readyz`, `/metrics`, structured JSON logs and optional OTLP traces. Correlate failures by response IDs. Never include access tokens, invitation tokens, raw webhook bodies or operator configuration in support reports. Metric labels must not contain user or tenant identifiers.

Event metering exposes `delcore_metering_backlog`, `delcore_metering_failed_jobs` and `delcore_metering_lag_seconds`. `GetMeteringStatus` returns the same lag and failure state for one tenant to its Owner and Billing users and reporting services, or service-wide to operators, with typed failure categories. Accepted events stay durable while processing is delayed; aggregates show the last applied state until the backlog drains. Failed `metering` jobs appear in `ListJobs` with a `metering_invalid`, `invalid_payload` or `retry_exhausted` category and return to processing through `RetryJob` after the cause is fixed. Pending or failed metering work holds final tenant deletion. Cardinality rejections leave no state behind; raise `MAX_METER_GROUPS` or `MAX_METER_GROUP_VALUES` and restart, or declare a new meter key with different grouping. The [recovery procedures](../../docs/servers-delcore-api-contract.md#recovery-procedures) cover invalid YAML, catalog constraints, database failure, backlog, exhausted retries, cardinality bounds and their operational effects, and restoring an earlier catalog definition.

Delcore is pre-release. The catalog and event metering change has no feature flag and no production data migration; its callers change with the server. Reset a development database instead of migrating it, as described in the [pre-release transition](../../docs/servers-delcore-api-contract.md#pre-release-transition).

Operators inspect failed jobs and audit history through authenticated APIs. Reprocessing requires a reason. An ambiguous payment returns to reconciliation, not a blind charge retry. If no provider transaction can be identified, inspect Paddle and preserve the failed job for review. Never change a balance directly in PostgreSQL; the ledger is append-only.

Refunds are initiated in Paddle. Allow signed adjustment events to synchronize the result. Deletion cannot finish while debt, a dispute, purchased balance or unresolved payment work remains. Do not delete the shared Logto identity.

Before an update:

1. Stop new traffic and terminate the server gracefully.
2. Create and verify a PostgreSQL backup using `pg_dump` and a disposable restore database. Protect the backup as sensitive data.
3. Preserve the current image reference and matching configuration separately.
4. Run the new image with `migrate` and inspect the result.
5. Start one new instance and verify readiness and operator access.

Use an older image only if it accepts the installed schema version. Otherwise stop the service, restore the verified backup into a separate database, restore matching configuration, and restart the compatible image. There is no arbitrary schema downgrade command. Reconcile provider events that arrived after the backup before reopening paid execution.

## Threat model

Trust boundaries are clients, Logto, consumer services, PostgreSQL, payment notifications, email delivery and operators. Check tenant scope and current roles on every protected operation. Reject user/M2M substitution, cross-service usage, privilege escalation, expired invitations, unauthorized redirect URLs and forged notifications. Signed events still require local ownership, currency, amount and item checks. Durable keys and transactional transitions protect against duplicate requests and restarts. Limit payloads, work duration and retries. Keep billing recovery available while new paid use is blocked.

A malicious authorized service can submit false usage for its own registered meters. Deployers must trust and monitor their reporting services. No reservation protocol prevents concurrent overspend. PostgreSQL and operator credentials are privileged; database-level administration can bypass application controls.

## Validation and packaging

```sh
cargo test --locked --manifest-path servers/Cargo.toml -p delcore-api
cargo clippy --locked --manifest-path servers/Cargo.toml -p delcore-api --all-targets -- -D warnings
cargo test --locked --manifest-path servers/Cargo.toml -p delcore-api -- --ignored
pnpm proto:lint
pnpm proto:generate
node protos/scripts/check-breaking.mjs "<comparison-revision>"
pnpm --filter @delinoio/delcore-api ci:oci
docker build -f "servers/delcore-api/Dockerfile" -t delcore-api:local .
```

The `ci:oci` command builds the image and runs disposable PostgreSQL and provider fixtures for startup, schema rejection, instance locking, payment crash recovery, graceful shutdown and backup restoration. It requires Docker. Rust integration tests require `TEST_DATABASE_URL` and fail if it is absent. They create isolated schemas in a disposable test database. Never point them at production. Provider fixtures and OCI builds do not establish live Logto, Paddle or Cloudflare account acceptance. The `delino` release publishes this image to GitHub Container Registry; see the [workflow contract](../../docs/repository-workflow-contract.md#server-image-publication). Deployment is not included.

Automatic top-ups require an active Paddle subscription. Tenants without a subscription use manual top-ups. Cancellation suspends automatic top-ups; payment recovery remains available through manual checkout.
