# Managed CPA operator monitoring

These assets monitor a Managed CPA (`mcpa-api`) deployment. They do not deploy the service, register the Delcore catalog or prove real provider, Logto or Delcore behavior. Thresholds are operational triggers, not availability, latency or capacity SLOs. Server assets use FSL-1.1-ALv2; ship `servers/LICENSE` and `servers/NOTICE` with them. The [server contract](../../../docs/servers-mcpa-api-contract.md) owns configuration, migrations, key rotation and recovery rules; this runbook covers monitoring only.

## Files

| File                 | Purpose                                                                                    |
| -------------------- | ------------------------------------------------------------------------------------------ |
| `thresholds.yml`     | Operator-owned alert thresholds, one constant series per threshold.                        |
| `recording.yml`      | Rolling windows over the server metrics. Counters are summed; replicated gauges use `max`. |
| `alerts.yml`         | Alert conditions and durations. Every comparison reads its value from `thresholds.yml`.    |
| `rules.test.yml`     | Simulated-time alert cases for the committed thresholds and durations.                     |
| `recording.test.yml` | Simulated-time cases for the recording windows and category filters.                       |
| `prometheus.yml`     | Example scrape and rule configuration with a Bearer metrics token file.                    |
| `dashboard.json`     | Grafana dashboard for calls, providers, Delcore, billing outbox, metering and budgets.     |
| `config.test.mjs`    | Invariants: thresholds come from `thresholds.yml`, queries use bounded labels only.        |
| `check.sh`           | Offline validation of all of the above.                                                    |

## Configure

1. Set `MCPA_METRICS_TOKEN` on the server. Put the same value in a secret file that Prometheus mounts at `/run/secrets/mcpa_metrics_token`. Never place the token in a scrape URL, dashboard or repository file.
2. Mount this directory at `/etc/mcpa-monitoring` and change the `mcpa-api` scrape target to the private service address. Keep `/metrics` off public routes. The alert rules expect `job="mcpa-api"`.
3. Choose deployment thresholds in `thresholds.yml`. Change alert durations in `alerts.yml` only together with `rules.test.yml`. Run `check.sh` before reloading Prometheus.
4. Import `dashboard.json` into Grafana and select the private Prometheus data source.
5. Route alerts through the deployment's Alertmanager. Severity `critical` affects billing correctness or availability; `warning` needs investigation; `info` is a signal for review.

## Alerts

| Alert                              | Initial condition (see `thresholds.yml`)                                                                                                         |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `McpaTargetDown`                   | A scrape target is down for 5 minutes.                                                                                                           |
| `McpaCallErrors`                   | More than 5% of model calls end `internal_error` or `rejected_dependency` in 5 minutes, with at least 20 calls, for 5 minutes. Holds 10 minutes. |
| `McpaProviderFailures`             | More than 25% of one provider's contacted calls fail with 5xx or network errors in 15 minutes, with at least 20 calls, for 5 minutes.            |
| `McpaDelcoreAuthorizationFailures` | More than 5 `Authorize`, `AuthorizeApiKey` or `GetTenantStates` failures (`unavailable` or `error`) per RPC in 5 minutes, for 2 minutes.         |
| `McpaOutboxBacklog`                | More than 1000 pending billing events for 10 minutes.                                                                                            |
| `McpaOutboxAge`                    | The oldest pending billing event is older than 900 seconds. Holds 10 minutes after recovery.                                                     |
| `McpaOutboxPoisoned`               | Any poisoned billing event exists.                                                                                                               |
| `McpaMeteringCheckpointFailures`   | Any metering checkpoint transaction failed in 5 minutes. Holds 5 minutes after recovery.                                                         |
| `McpaBudgetRejections`             | More than 100 budget rejections per scope in 15 minutes, for 5 minutes.                                                                          |

Equality is not a breach. Low samples cannot fire a ratio alert. Provider 429, other 4xx and authentication failures belong to customer-owned accounts; they appear on the dashboard but do not alert. Delcore `denied` results are authorization decisions, not failures. Missing metrics are not success: restore the scrape before treating an alert as resolved.

## Metrics

The server exports these families with bounded labels only. No metric carries tenant, resource, call key, model or user identities, request bodies or secrets.

- `mcpa_calls_total{format,provider,status}` and `mcpa_call_duration_seconds{format,status}`. `status` is the lowercase `CallStatus` name without its prefix, for example `succeeded`, `provider_error`, `internal_error` or `rejected_dependency`.
- `mcpa_provider_failures_total{provider,status_class}` with `429`, `4xx`, `5xx`, `auth` or `network`.
- `mcpa_delcore_failures_total{rpc,kind}` with the Delcore RPC method name and `unavailable`, `denied` or `error`.
- `mcpa_outbox_pending`, `mcpa_outbox_poisoned` and `mcpa_outbox_oldest_pending_age_seconds` (zero when empty). These are global PostgreSQL projections repeated by each replica; use `max`, never `sum`.
- `mcpa_outbox_delivery_failures_total{category}`, `mcpa_metering_checkpoint_failures_total`, `mcpa_budget_rejections_total{scope}`, `mcpa_billable_bytes_total{segment}` and `mcpa_http_requests_total{surface,code_class}`.
- `mcpa_runtimes_loaded` is the number of resource runtimes this replica holds in memory. It rises with the first call of each resource and falls when the janitor evicts runtimes idle for 30 minutes or above 5000 loaded runtimes. Each replica reports its own value; use `max` or `sum` by intent. A value that stays at 5000 means more resources are in active use than the cap and calls rebuild runtimes often.

## Responses

### Target down or call errors

Check `/healthz`, `/readyz`, the deployed revision and the structured logs by request ID. `/readyz` fails when the database is unreachable or the schema is not the expected version. `serve` never migrates: apply migrations explicitly with `mcpa-api migrate`, then confirm `mcpa-api check-schema`. Correct a defect by deploying a fixed version. Do not roll back to a previous image or binary and do not write reverse migrations.

### Provider failures

Compare the failing provider across resources on the dashboard. mcpa does not retry across accounts or providers, so provider errors reach callers directly. Confirm egress connectivity and the provider's status. Do not move traffic to other customers' accounts or add service-owned accounts.

### Delcore authorization failures

Model calls fail closed after the authorization cache expires and management returns a dependency error. Restore Delcore reachability and the M2M credentials (`LOGTO_TOKEN_URL`, `CLIENT_ID`, `CLIENT_SECRET`, `DELCORE_RESOURCE`). Never bypass authorization, extend the cache TTL to hide the outage or cache denials.

### Billing outbox backlog, age or poisoned events

Pending events retry until delivered and expire 30 days after creation; expired events are deleted without estimating. Restore Delcore `IngestEvent` delivery first. Event keys are idempotent, so delivery after recovery bills each checkpoint once. A poisoned event received a non-retryable Delcore reason: inspect the reason category, correct the Delcore catalog or registration, and deploy a fix. Do not edit, replay by hand or delete outbox rows to clear the alert.

### Metering checkpoint failures

A failed checkpoint leaves measured bytes uncommitted; a crash before the next successful checkpoint loses them from billing. Check database health and transaction errors in the logs. Restore the database before the next deployment restart.

### Budget rejections

Rejections enforce customer limits and are expected. A surge across many tenants can indicate a wrong billing period or price; compare the tenant's Delcore period with the budget view before changing anything.

## Offline validation and remaining acceptance

Run `servers/mcpa-api/ops/check.sh` from the checkout. It checks rule syntax, runs the simulated-time rule tests, checks the example scrape configuration syntax and runs the invariant tests, using the pinned `prom/prometheus:v3.15.1` image. Set `PROMTOOL` to a local promtool binary of the same version to run without Docker. No server, provider or alert receiver starts.

The rule tests are not a live scrape, alert delivery, Grafana rendering, service rollout or Delcore billing acceptance. Record exact revisions, commands, results and those unperformed checks in review or CI evidence. See the [Prometheus alerting rule reference](https://prometheus.io/docs/prometheus/latest/configuration/alerting_rules/) and [rule unit testing](https://prometheus.io/docs/prometheus/latest/configuration/unit_testing_rules/).
