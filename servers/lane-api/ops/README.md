# Lane operator runbooks

These assets configure monitoring for a Lane API deployment. They do not deploy a production service or demonstrate real Logto, push, or email delivery. Thresholds are operational triggers, not availability, latency, or capacity SLOs. Server assets use FSL-1.1-ALv2; ship `servers/LICENSE` and `servers/NOTICE` with them.

## Configure and start

Build the owning server OCI artifact and retain its source revision. Provision PostgreSQL with encrypted storage and backups, a private service network, and TLS for `https://lane.api.delino.io`. Keep `/metrics`, monitoring ports, and secrets off public routes. Serve the separately built online web artifact at `https://lane.delino.io`. CORS must allow only configured frontend origins.

Use the server's `.env.example` as the authoritative configuration-name inventory. Inject secrets through the deployment secret manager; never commit a real environment file. Required settings include:

- `LISTEN` (development `127.0.0.1:46326`; use the private container interface for a container), `LANE_PUBLIC_URL`, `WEB_ORIGIN`, and `DATABASE_URL`.
- Private image storage: `LANE_R2_ENDPOINT`, `LANE_R2_BUCKET`, `LANE_R2_ACCESS_KEY_ID`, and `LANE_R2_SECRET_ACCESS_KEY`. Use a bucket-scoped R2 token and disable public bucket access. See the [server attachment contract](../../../docs/servers-lane-api-contract.md#private-image-persistence-and-operations) for CORS, transfer expiry, decoder limits, deletion queues and recovery.
- `LANE_METRICS_TOKEN`, at least 32 characters. The metrics endpoint requires Bearer authentication. Put the same value in the separate Prometheus secret file; never place it in a scrape URL or dashboard.
- Positive `MAX_ITEM_BYTES`, `MAX_WEBHOOK_BYTES`, `REQUESTS_PER_MINUTE`, `WEBHOOK_REQUESTS_PER_MINUTE`, and `AUTH_REQUESTS_PER_MINUTE`. There are no defaults. Select values before startup; missing/invalid limits must fail startup. These limits do not apply to standalone local input.
- `LOGTO_ISSUER`, `LANE_LOGTO_CLIENT_ID`, `LANE_LOGTO_CLIENT_SECRET`, `DELCORE_RESOURCE`, `DELCORE_URL`, `LOGTO_TOKEN_URL`, and M2M `CLIENT_ID`/`CLIENT_SECRET`. The confidential Lane client's callback is the exact public URL plus `/auth/callback`.
- `LANE_WEB_RETURN_URLS`: a JSON array of exact same-site frontend return URLs. `LANE_OAUTH_CLIENTS`: a JSON map from client ID to `name`, exact `redirect_uris`, `origin_kind` (`ORIGIN_KIND_CLI`, `ORIGIN_KIND_APP`, or `ORIGIN_KIND_MCP`), and `resource` (API URL or API URL plus `/mcp`). Keep audiences distinct; an MCP token is not a Delcore-resource token.
- `LANE_WEBHOOK_ENCRYPTION_KEY`: base64url-encoded 32-byte encryption key, and `LANE_WEBHOOK_BASE_URL=https://lane.api.delino.io`. Retain encryption keys securely for previously accepted encrypted data; do not replace one casually during incident response.

Register Lane in Delcore with `authorize` scope and `manage` capability. Authorization uses the personal tenant, `paid=false`, and ACTIVE state. Do not substitute an organization tenant or relax checks to recover from dependency failure.

Apply migrations explicitly with `lane-api migrate`, then run `lane-api check-schema`, then `lane-api serve`. Startup must not apply migrations. Run migration commands with the intended database configuration, review their result, and retain revision/command/result evidence without credentials. Validate `/health/live` and `/health/ready` before routing traffic. Readiness includes DB access; readiness failures can also reflect other dependencies, so confirm the failing dependency before diagnosis.

## Deploy monitoring

`compose.yml` runs monitoring only on an existing private network. Configure `LANE_MONITORING_NETWORK`, `LANE_METRICS_TOKEN_FILE`, `LANE_SMTP_PASSWORD_FILE`, and `LANE_ALERTMANAGER_CONFIG` as absolute operator-owned paths where applicable. Prepare the Alertmanager file from `alertmanager.example.yml`: replace reserved SMTP sender/recipient/host values, preserve TLS, and mount the password at `/run/secrets/lane_smtp_password`. No email provider integration belongs in Lane.

Review `prometheus.yml` targets and port for your private network. Mount the directory at `/etc/lane-monitoring`; Prometheus reads the metrics token from `/run/secrets/lane_metrics_token`. `blackbox.yml` probes live/readiness every 30 seconds with a five-second timeout. The exporter scrape allows six seconds for response overhead, so its timeout offset does not shorten the probe. Confirm the exporter can reach both URLs. Keep the external network name and actual secret files out of Git.

Run `docker compose -f servers/lane-api/ops/compose.yml config` to validate your prepared configuration before an authorized rollout. Starting this composition is a deployment action, separate from the offline validation below. Persist the Prometheus and Alertmanager volumes across restarts to retain observations and notification deduplication. Import `dashboard.json` into Grafana and select the private Prometheus data source. It shows probes, response/error rates, p95, minimum-sample counts, push queue/attempts, synchronization failures and firing alerts. Dashboard queries contain no account IDs, task text, payloads or credentials.

## Alert semantics and changes

`recording.yml` computes rolling rates; `alerts.yml` defines thresholds/durations. These files and the scrape, blackbox and Alertmanager configuration are operator-configurable. Change related values together, update rule tests, validate, and reload using the monitoring service's supported procedure.

| Signal                  | Initial firing condition                                                  | Recovery evidence                                                     |
| ----------------------- | ------------------------------------------------------------------------- | --------------------------------------------------------------------- |
| Service/DB probe        | Five continuous minutes failing; probe every 30 s, timeout 5 s            | Five continuous minutes successful probes and healthy exporter scrape |
| API errors/timeouts     | More than 5% in rolling 5 min, at least 20 requests, continuously 5 min   | At most 5% with at least 20 requests, continuously 10 min             |
| API p95                 | More than 2 s in rolling 5 min, at least 20 requests, continuously 5 min  | At most 2 s with at least 20 requests, continuously 10 min            |
| Transient push failures | More than 10% in rolling 15 min, at least 20 attempts, continuously 5 min | At most 10% with at least 20 attempts, continuously 10 min            |
| Oldest pending push     | More than 600 s; no additional firing delay                               | Age at most 600 s or queue empty, continuously 10 min                 |

Equality is not a threshold breach. Low samples cannot fire a new rate alert and cannot prove recovery of an existing one. The alert expression retains firing state when samples are insufficient or metrics disappear; `keep_firing_for` requires continuous positive recovery evidence. A return to insufficient samples interrupts the recovery period. An empty push queue clears only its age condition, not an independently firing failure-rate alert. Restore metrics if missing; do not interpret absence as success. A fresh Prometheus installation has no prior firing state; retain its storage and verify active incidents after a monitoring outage.

Alertmanager disables grouping across distinct alerts, sends once when firing, repeats hourly, and sends once on recovery. `group_wait=0s`, `group_interval=30s`, `repeat_interval=1h`, and `send_resolved=true` implement this routing. Network/email delivery can add delay; routing does not prove delivery. Silence only with an operator-recorded reason and expiry. Do not lower thresholds merely to hide an incident.

Metrics must use bounded operational labels. `lane_api_requests_total{outcome="success|client_error|server_error|timeout"}` initializes all outcome counters. `lane_api_request_duration_seconds` excludes long-lived subscription lifetime and initializes its buckets. `lane_push_attempts_total{result="success|transient_failure|permanent_failure"}` initializes all result counters. Export `lane_push_pending_count`, `lane_push_oldest_pending_age_seconds` (zero when empty), and `lane_sync_pending_failures`. Push counters and queue/sync gauges are global PostgreSQL projections repeated by each replica, including unclaimed/inflight deliveries. Use max across replicas grouped by service (and result before summing push attempt categories within each service); never sum replicated values. API counters/histograms are process-local and are summed within each service. Set the same bounded service label on each replica scrape; different services must retain different labels. A healthy replica retains the global view after failover. Do not label metrics with account/resource IDs or user content.

## Service and database outage

1. Check liveness, readiness, exporter reachability and the current deployment revision. Use safe correlation IDs and structured error categories to distinguish process, network, DB and Delcore failures.
2. Restore private connectivity and database availability. Verify schema compatibility and configured credentials without printing them. Do not recreate an existing database or apply down migrations as a recovery shortcut.
3. If a binary regression caused the failure, roll back only to a version compatible with the current schema. Otherwise deploy a corrected forward version after explicit migration review. Recheck schema, readiness and representative mocked/read-only operations before traffic recovery.
4. Preserve local clients' pending changes. Confirm sufficient healthy observations and alert recovery; do not manually erase incident state to manufacture recovery.

Expire server logs after 30 days using the deployment log collector's retention policy. Log only operation/result/duration and correlation metadata. Never log tokens, receiver secrets, task/prompt content or webhook bodies. Monitoring does not override content retention: prompts/history/events remain until explicit owning deletion.

## Authentication and synchronization

Verify Logto issuer/audiences/redirects, M2M acquisition, Delcore reachability and personal-tenant ACTIVE status. An unavailable Delcore dependency is an unavailable operation, not proof that the caller's token is invalid. Do not bypass authorization or clear grants to fix it. Revoked devices/grants require explicit new approval; offline copies may remain.

Inspect pending-sync counts and safe per-item rejection reasons. An oversized new/modified item should block only itself. Lower limits must still allow reads/downloads/deletes of accepted content. Raise a mistaken limit through reviewed configuration or let the user edit the item; never drop the outbox. Unsupported sync/local schema needs a compatible version, not empty-store initialization. Conflicts need explicit user/agent resolution; operators must not choose a timestamp winner. Account deletion/reconnection must preserve epochs so old content cannot reappear.

## Webhooks and leases

Check receiver existence, repository ownership, current key, sender time and raw-byte signing without logging the secret/body. `Lane-Timestamp` is canonical decimal Unix seconds; `Lane-Signature` is `v1=` plus lowercase HMAC-SHA256 hex over timestamp, period, exact body. Decode the base64url receiver secret before HMAC. Both ±300 s boundaries pass; ±301 s fails. Freshly sign retries with the original event ID/content. Rotation invalidates the old key immediately. A lost secret response requires rotation.

Same event identity with changed content is a conflict; deleted receipts must not recreate payloads. Reading an event does not complete it. A lease lasts five minutes; expiry is exclusive (`now < expires_at`), and renewal must occur before expiry. Released/expired tokens cannot complete. Response-loss completion retries reuse identical request ID/content; receipts return the original result. Completion must atomically create zero or more tasks and a processing record. Never manually insert tasks to repair a failed transaction. Reopen preserves earlier records. Event/repository deletion invalidates leases; receiver deletion alone does not.

## Push delivery

Enable only providers with configured credentials. Missing credentials for an enabled provider fail startup; disabled providers do not send.

- APNs: `LANE_APNS_ENABLED`, `LANE_APNS_PRIVATE_KEY` (PEM), `LANE_APNS_KEY_ID`, `LANE_APNS_TEAM_ID`, `LANE_APNS_TOPIC`, and `LANE_APNS_SANDBOX`.
- FCM: `LANE_FCM_ENABLED`, `LANE_FCM_PRIVATE_KEY` (PEM), `LANE_FCM_CLIENT_EMAIL`, and `LANE_FCM_PROJECT_ID`.
- Web Push: `LANE_WEB_PUSH_ENABLED`, `LANE_WEB_PUSH_PRIVATE_KEY` (PEM), and `LANE_WEB_PUSH_SUBJECT` (mailto/HTTPS).

APNs sends background data-only with content-available 1 and priority 5. FCM sends data-only with HIGH priority. Native clients synchronize, deduplicate receipts and display local notifications; iOS wakes are platform-controlled and may wait for later permitted sync/app opening. Web Push is displayed by the service worker. There is no WNS integration. Unsupported environments retain in-app indications and available local notifications.

Only new MCP-created pending proposals enqueue notifications. Check per-device opt-in, permission, provider support, deduplication, origin-device exclusion, and current proposal state. Acceptance/rejection/deletion cancels unsent work. Permanent failures stop. After an initial transient failure, retry after 30, 120, 600, 1800 and 3600 seconds from the preceding failed attempt, at most five retries. Honor delta/date Retry-After by taking the later deadline. Expire at creation plus two hours; do not send at/after expiry or when Retry-After exceeds it. Do not reset creation time to extend a backlog.

## Offline validation and remaining acceptance

Run `servers/lane-api/ops/check.sh` from the checkout. It checks Prometheus rules, simulated-time rule cases, Alertmanager syntax and routing/probe invariants with pinned tool images. No server, provider or mail service starts. Tests cover exact thresholds, minimum samples, delayed firing/recovery and interrupted recovery. Review deployment-specific configuration with `promtool check config` and `amtool check-config` after mounting its secrets and targets.

The rule engine tests are not live scrape, SMTP receipt, Grafana rendering, service rollout or provider/device acceptance. Record exact revisions, commands/results and those unperformed checks in review/CI evidence. Real credential/provider checks require separate authorization. Official configuration references: [Prometheus alert rules](https://prometheus.io/docs/prometheus/latest/configuration/alerting_rules/) and [Alertmanager routing](https://prometheus.io/docs/alerting/latest/configuration/).
