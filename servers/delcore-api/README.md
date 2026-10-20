# Delcore API

Delcore owns shared personal and organization tenants, service authorization, usage accounting and tenant-wide billing. It runs a Connect API and durable background jobs in one Go process with PostgreSQL. It has no UI or separate SDK package.

The [project index](../../docs/project-delcore.md), [server contract](../../docs/servers-delcore-api-contract.md) and [protocol contract](../../docs/protos-delcore-v1-contract.md) define the full behavior contract. Repository-owned files in `servers/` use [FSL-1.1-ALv2](../LICENSE).

## Operator prerequisites

- A PostgreSQL database dedicated to this service, with a separate backup destination. Use a migration role that can create schema objects. Restrict application access to the owning database.
- An operator-owned Logto issuer, API audience, dedicated operator role, and distinct M2M applications for each service. User access tokens must include a verified email claim. Never use an ID token as an API access token. Register each M2M client and its allowed scopes through the operator API.
- A Paddle Billing account with monthly recurring prices, an appropriate product for purchased usage credits, an API token, and a signed notification destination at `/webhooks/paddle`. Subscribe to transaction completion/failure, subscription lifecycle and adjustment creation/update events. Use sandbox credentials for validation.
- Cloudflare Email Sending enabled for the sender domain, an account-scoped sending token, and valid sender DNS configuration. Email Sending remains beta. A Workers Paid plan is required for arbitrary recipients; consult the [current setup guide](https://developers.cloudflare.com/email-service/get-started/). The Go server uses REST, not a Workers binding.
- Consumer application pages for invitation acceptance and payment return. Register their exact URLs. Delcore does not supply these screens.
- A TLS reverse proxy for public API access. Expose health and metrics endpoints only to trusted monitoring infrastructure. Do not publish PostgreSQL.

## Configuration and startup

Copy `.env.example` to an uncommitted `.env` in the service working directory, replace every placeholder, restrict file permissions and choose policy limits for your deployment. The example retention periods are examples, not a statement of legal requirements. Keep the identity key stable across updates: it derives the stored identity reference. Loss or unplanned rotation prevents identity matching.

```sh
go run ./cmd/delcore-api -migrate
go run ./cmd/delcore-api
```

Run these commands from `servers/delcore-api`. Migrations are explicit and transactional. Normal startup checks schema compatibility; it does not apply migrations. A database session lock rejects a second running instance. The service stops if it loses that lock connection.

With `DEVELOPMENT=true`, normal startup reads the [split-file YAML catalog](catalog) after the schema check and instance lock, then reconciles declared entities in one transaction before serving requests or processing jobs. The default is `catalog` from this working directory. Set `CATALOG_DIR` to another existing directory to use your own declarations. It scans nested `.yaml` and `.yml` files, with one entity per file; absent, empty, malformed, or inconsistent catalogs stop startup without partial changes. The examples declare a service, meter, features, plan, add-on, and prices in separate files. Meters and features use service keys; add-on prices name both the owning service and add-on key. See the [development catalog contract](../../docs/servers-delcore-api-contract.md#development-catalog) for the full schema, literal semantics, and reference rules.

Files restore declared fields changed through operator APIs. Omitted entities remain in the database. Unchanged content reuses stable IDs and versions, and returning from definition A to B to A reselects A for future subscriptions without moving existing subscriptions. To recover from invalid YAML or a catalog constraint, correct the files or conflicting declaration and restart. If PostgreSQL is unavailable, restore database service first. YAML edits alone do not trigger Air; restart the API task. Production startup and `-migrate` do not read the catalog. Keep real credentials out of catalog files.

Bootstrap the service catalog and default free plan with a user token carrying the dedicated operator role. Create registered meters before creating usage prices. Register all prices before marking the free plan as the default. Price versions are immutable. Existing entitlements refer to their chosen price version. Keep fixed recurring prices separate from usage prices.

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
| `DEVELOPMENT`                                                 | Permit loopback HTTP provider URLs for local fixtures  |
| `CATALOG_DIR`                                                 | Optional development YAML catalog directory            |

All billing uses USD with two decimal places for currency minor units. Currency and minor-unit precision are fixed and have no configuration settings.

The server reads individual environment variables, with process variables taking priority over the optional working-directory `.env`, including explicitly empty values. `RETURN_URLS` is a JSON array of exact URL strings. Integer settings use base-10 values. `DEVELOPMENT` accepts Go boolean values and defaults to false. The loader rejects invalid values, missing required settings and contradictory policies. The `CONFIG` variable and `-config` flag are no longer supported. Optional standard OTLP environment variables configure tracing.

## Consumer API

Generate clients from the `delcore.v1` schemas in [Delino Apps](https://github.com/delinoio/apps) or use the generated Go Connect client. Every mutation supplies a UUID v7 `request_key`. Repeating a key with different input returns a conflict. Responses and typed errors carry a correlation ID.

The sample client reads a token from the environment and prints only tenant count and correlation metadata. From the repository root:

```sh
go run ./examples/delcore-client -endpoint "http://127.0.0.1:8080"
```

User clients discover their personal tenant, create organizations, manage invitations and groups, and inspect billing through their current roles. Owner transfer requires acceptance by the receiving member. Admin has no implicit billing authority and cannot grant privileged roles.

M2M clients call `Authorize`, `ReportUsage`, `CorrectUsage` and service-scoped `ListUsage` under their registered service identity. Treat an unavailable authorization decision as a denial of new paid execution. Retain pending usage reports locally until accepted. Use a stable event key for every measured execution. A correction supplies the original tenant, meter and occurrence time, references the previous event, and replaces its quantity. Corrections and ledger effects append new records.

Post-execution reporting cannot guarantee zero overspend under concurrency. A non-postpaid tenant may incur negative credits; the next top-up offsets that debt. Monetary values are decimal strings. Prorated included quantities are floored to 0.000001; monetary products keep full precision until cumulative service/meter period totals are rounded half-up to currency minor units. Occurrence timestamps are normalized to PostgreSQL microsecond precision. Credits pay usage only. Fixed subscription and add-on fees remain Paddle charges. Negative postpaid corrections carry forward against later usage. On cancellation, refund any remaining usage overpayment through Paddle. Select the original usage transaction item; an approved refund clears the settlement hold after webhook processing. A fixed subscription fee refund does not clear usage debt. Keep the transaction item attribution when restoring a backup; an unverified refund must remain under operator review.

Payment requests return a durable job ID. Poll `ListBillingHistory` for payment state and checkout URLs. A checkout return is not proof of payment. One-time charges use Paddle price metadata to correlate the resulting transaction; the subscription response itself never grants credits. Recover payment failures through the payment provider and inspect independently enforced blocking reasons.

## Operations and recovery

Use `/healthz`, `/readyz`, `/metrics`, structured JSON logs and optional OTLP traces. Correlate failures by response IDs. Never include access tokens, invitation tokens, raw webhook bodies or operator configuration in support reports. Metric labels must not contain user or tenant identifiers.

Operators inspect failed jobs and audit history through authenticated APIs. Reprocessing requires a reason. An ambiguous payment returns to reconciliation, not a blind charge retry. If no provider transaction can be identified, inspect Paddle and preserve the failed job for review. Never change a balance directly in PostgreSQL; the ledger is append-only.

Refunds are initiated in Paddle. Allow signed adjustment events to synchronize the result. Deletion cannot finish while debt, a dispute, purchased balance or unresolved payment work remains. Do not delete the shared Logto identity.

Before an update:

1. Stop new traffic and terminate the server gracefully.
2. Create and verify a PostgreSQL backup using `pg_dump` and a disposable restore database. Protect the backup as sensitive data.
3. Preserve the current image reference and matching configuration separately.
4. Run the new image with `-migrate` and inspect the result.
5. Start one new instance and verify readiness and operator access.

Use an older image only if it accepts the installed schema version. Otherwise stop the service, restore the verified backup into a separate database, restore matching configuration, and restart the compatible image. There is no arbitrary schema downgrade command. Reconcile provider events that arrived after the backup before reopening paid execution.

## Threat model

Trust boundaries are clients, Logto, consumer services, PostgreSQL, payment notifications, email delivery and operators. Check tenant scope and current roles on every protected operation. Reject user/M2M substitution, cross-service usage, privilege escalation, expired invitations, unauthorized redirect URLs and forged notifications. Signed events still require local ownership, currency, amount and item checks. Durable keys and transactional transitions protect against duplicate requests and restarts. Limit payloads, work duration and retries. Keep billing recovery available while new paid use is blocked.

A malicious authorized service can submit false usage for its own registered meters. Deployers must trust and monitor their reporting services. No reservation protocol prevents concurrent overspend. PostgreSQL and operator credentials are privileged; database-level administration can bypass application controls.

## Validation and packaging

```sh
go test -count=1 ./servers/delcore-api/...
go vet ./servers/delcore-api/...
go test -count=1 -race -tags=integration ./servers/delcore-api/...
pnpm proto:lint
pnpm proto:generate
node scripts/check-proto-breaking.mjs "<comparison-revision>"
pnpm --filter @delinoio/delcore-api ci:oci
docker build -f "servers/delcore-api/Dockerfile" -t delcore-api:local .
```

Integration tests require `TEST_DATABASE_URL` and fail if it is absent. They create isolated schemas in a disposable test database. Never point them at production. Provider fixtures and OCI builds do not establish live Logto, Paddle or Cloudflare account acceptance. No deployment infrastructure or image publication is included.

Automatic top-ups require an active Paddle subscription. Tenants without a subscription use manual top-ups. Cancellation suspends automatic top-ups; payment recovery remains available through manual checkout.
