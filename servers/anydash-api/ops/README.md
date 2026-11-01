# AnyDash operator runbooks

These assets configure monitoring for an AnyDash API deployment. They do not deploy a production service or demonstrate real Logto, Delcore, provider or email delivery. Thresholds are operational triggers, not SLOs; AnyDash is best effort. Server assets use FSL-1.1-ALv2; ship `servers/LICENSE` and `servers/NOTICE` with them.

## Startup and configuration

Build the OCI image from the locked server workspace and retain its source revision. Provision PostgreSQL 17 with encrypted storage and backups whose retention is at most 30 days, a private service network, and TLS for `https://anydash.api.delino.io`. Keep `/metrics`, monitoring ports and secrets off public routes. The static web client is served separately at `https://anydash.delino.io`; CORS allows only `WEB_ORIGIN`.

Use the server's `.env.example` as the authoritative configuration-name inventory. Inject secrets through the deployment secret manager; never commit a real environment file. Invalid or missing required values stop startup.

- `LISTEN`, `DATABASE_URL`, `ANYDASH_PUBLIC_URL` and `WEB_ORIGIN` (both origins without a path).
- `LOGTO_ISSUER` and `ANYDASH_RESOURCE` validate user tokens; the signing keys are read from `<LOGTO_ISSUER>/jwks`. `DELCORE_RESOURCE` validates the Delcore token that clients send to `ConnectAccount` and `ListSpaces`.
- `DELCORE_URL`, `LOGTO_TOKEN_URL`, `CLIENT_ID` and `CLIENT_SECRET`: the machine-to-machine identity. Register AnyDash in the Delcore catalog as a service with the `authorize` scope; it calls `GetTenantStates` and `GetTenantMemberRoles` only.
- `ANYDASH_MIN_CLIENT_VERSIONS` (required, every client kind) and `ANYDASH_LATEST_CLIENT_VERSIONS` (optional); see the [minimum client version procedure](#minimum-client-version).
- `ANYDASH_METRICS_TOKEN`, at least 32 characters. Put the same value in the Prometheus secret file; never place it in a scrape URL or dashboard.
- `ANYDASH_CLIENT_IP_HEADER` (optional): the header a trusted reverse proxy sets to the client address, such as `CF-Connecting-IP`. The unauthenticated `StartEnrollment` limit (10 per minute) is keyed by this address. Leave it empty when clients connect directly; never set it when untrusted clients can reach the API without the proxy, because they could choose their own address.
- `ANYDASH_PRICING_URL` (optional) overrides the LiteLLM pricing JSON URL. `DEPENDENCY_TIMEOUT` (default `5s`) bounds Delcore and Logto calls.

## Explicit migration

Apply migrations with `anydash-api migrate`, then run `anydash-api check-schema`, then `anydash-api serve`. Serving verifies the schema version and refuses a missing, older, newer or modified schema; it never migrates. Run migration commands with the intended database configuration, review the result and retain revision, command and result evidence without credentials. Validate `/health/live` and `/health/ready` before routing traffic.

## Fix-forward recovery

1. Check liveness, readiness and the deployment revision. Use correlation IDs from client error screens or `anydash status` to find the request log; logs carry the correlation ID, method, client kind and version, result and `ErrorReason`, never tokens, codes or usage values.
2. Restore private connectivity and database availability. Do not recreate an existing database as a recovery shortcut.
3. For a binary regression, deploy a corrected forward release. If persisted state needs correction, apply a new reviewed forward migration. Never roll back to a previous image or binary and never write or apply down migrations.
4. Confirm sufficient healthy observations and alert recovery; never erase incident state to manufacture recovery.

## Delcore and Logto outages

Every protected request reads tenant state and team roles from Delcore within the request; there is no authorization cache. A Delcore or Logto key outage returns `DEPENDENCY_UNAVAILABLE` (HTTP 503, counted as a server error) and denies access. It is never reported as an invalid caller credential, and collectors keep buffering locally. Do not bypass authorization to recover. Verify the machine-to-machine identity, the `authorize` scope, network reachability and the Logto JWKS endpoint. The hourly `tenant_lifecycle` job skips cleanup while Delcore is unavailable; only an explicit `DELETED` state triggers withdrawal or team dashboard deletion.

## Deletion backlog

Connection, device, dashboard and account data deletion, user withdrawal and tenant deletion block access immediately and enqueue a deletion job with a deadline 24 hours after the request. The `deletion` job runs every minute. `AnyDashDeletionDeadlineAtRisk` fires when a job is still pending 20 hours after its request, and `AnyDashWorkerJobFailed` fires when a deletion or retention run fails. Check the database, the job's `failure_category` and `attempts` in `deletion_jobs`, and the worker logs by correlation ID. Fix the cause forward; never mark a job complete by hand without deleting its data. Backup copies expire with the backup retention period, which is at most 30 days.

## Price sync failure

The `price_sync` job fetches the LiteLLM pricing JSON daily at 03:00 UTC with a 30-second timeout and retries hourly after a failure. A failed fetch or a document that fails validation keeps the previous price version in use and is recorded in `price_sync_runs`. `AnyDashPriceValidationFailed` fires for a rejected document and `AnyDashPriceSyncStale` after 48 hours without a successful sync. Check the `failure_category` of the latest runs (`fetch`, `status`, `too_large`, `invalid_json`, `not_object`, `too_few_models`, `invalid_entry`, `invalid_model_key`, `invalid_price`) and the upstream document. Past costs are never recalculated; a new version only applies to usage at or after its `effective_from`.

## Provider endpoint changes

The Codex `wham/usage` endpoint, Claude `/api/oauth/usage` and Grok `x.ai/billing` are undocumented and can change without notice. The parser and provider alerts are the detection path: `AnyDashProviderCollectionFailures` fires when a provider's `FAILED` or `RATE_LIMITED` snapshots exceed 20% in one hour, and `AnyDashPartialAggregation` fires when 5% or more of the Codex or Claude Code devices that uploaded in one hour report `PARTIAL`. `AUTH_EXPIRED` and `NOT_PROVIDED` are user or provider states, not failures. The response is a fix-forward collector release; the server keeps accepting older collectors at or above the minimum version.

## Minimum client version

`ANYDASH_MIN_CLIENT_VERSIONS` sets the minimum SemVer for every `ClientKind`. The check runs before authentication on every API request; older clients receive `UPDATE_REQUIRED` and collectors keep aggregating locally for up to 90 days. To raise a minimum: release the new client first, set `ANYDASH_LATEST_CLIENT_VERSIONS` so that heartbeats announce the update, wait for adoption, then raise the minimum through a reviewed configuration change and restart. Never raise a minimum above a published release, and lower it again by the same procedure if needed.

## Deploy monitoring

`compose.yml` runs Prometheus and Alertmanager on an existing private network. Configure `ANYDASH_MONITORING_NETWORK`, `ANYDASH_METRICS_TOKEN_FILE`, `ANYDASH_SMTP_PASSWORD_FILE` and `ANYDASH_ALERTMANAGER_CONFIG` as absolute operator-owned paths. Prepare the Alertmanager file from `alertmanager.example.yml`. Import `dashboard.json` into Grafana and select the private Prometheus data source.

Metrics use bounded labels only: Connect service and method names, outcomes, worker job names, providers, collection statuses, tools and parser states. The worker label is `worker_job` because `job` is the Prometheus target label. Request counters and histograms are process-local and summed; gauges backed by PostgreSQL (`anydash_deletion_*`, `anydash_price_sync_last_success_timestamp_seconds`, `anydash_parser_devices`, `anydash_leases_active`, `anydash_worker_job_last_success_timestamp_seconds`) are repeated by every replica and use max across replicas.

| Alert                               | Initial firing condition                                                              |
| ----------------------------------- | ------------------------------------------------------------------------------------- |
| `AnyDashApiServerErrors`            | Server errors above 2% of at least 20 requests in 5 minutes, for 5 minutes            |
| `AnyDashWorkerJobFailed`            | A `retention` or `deletion` run failed in the last hour                               |
| `AnyDashDeletionDeadlineAtRisk`     | The oldest pending deletion job is more than 20 hours old                             |
| `AnyDashPriceSyncStale`             | No successful LiteLLM price sync for 48 hours                                         |
| `AnyDashPriceValidationFailed`      | A LiteLLM document failed validation in the last hour                                 |
| `AnyDashProviderCollectionFailures` | `FAILED` or `RATE_LIMITED` snapshots above 20% of at least 20 in 1 hour, per provider |
| `AnyDashPartialAggregation`         | 5% or more of uploading Codex or Claude Code devices report `PARTIAL`                 |

## Offline validation

Run `servers/anydash-api/ops/check.sh` from the checkout. It checks the rules, simulated-time rule tests, Alertmanager syntax, Prometheus syntax and configuration invariants with pinned tool images. The rule tests are not live scrape, email receipt, Grafana rendering, service rollout or provider acceptance.
