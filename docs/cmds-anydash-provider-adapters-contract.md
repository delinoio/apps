# AnyDash Provider Adapters Contract

## Scope

This contract owns the AnyDash provider matrix, the general metric rules and the provider adapters in the Rust collector under `cmds/anydash` (`implemented`). Each adapter is implemented against this matrix. It covers subscription usage (Codex, Claude Code, Grok), local session usage (Codex, Claude Code) and read-only deployment platforms (Vercel, Supabase, Cloudflare). The [CLI contract](cmds-anydash-cli-contract.md) owns scheduling, backoff, credential storage and session file parsing mechanics; the [protocol contract](protos-anydash-v1-contract.md) owns `MetricKind`, `MetricState` and the snapshot messages.

## Runtime and Language

Rust in the root Cargo workspace, as part of the `anydash` executable. Adapters use one shared HTTP client with a 30-second request timeout, TLS certificate verification and no cookie store. Adapters never use browser sessions or cookies. Rust is a documented exception to the repository's Go default for non-server projects; see the [project index](project-anydash.md#platforms-and-deviations-from-repository-defaults).

## Users and Operators

Collector devices run adapters for the connections whose lease they hold and for local session files. Users supply official tokens or sign in to the provider's own CLI. The maintainer validates every provider with real accounts before release.

## Interfaces and Contracts

### General metric rules

- If a provider does not offer a metric, the connection is still allowed and the metric shows "Not provided" (`METRIC_STATE_NOT_PROVIDED`; final copy comes from design approval). Adapters report every metric of the matrix for a connection on every successful cycle, with `NOT_PROVIDED` where applicable, so clients can distinguish "Not provided" from a temporary collection failure (`FAILED`).
- Subscription remaining is shown only as `100% − provider-reported used percentage`. The server derives `SUBSCRIPTION_REMAINING_PERCENT`; adapters report only `SUBSCRIPTION_USED_PERCENT`. Absolute remaining amounts (`SUBSCRIPTION_REMAINING_AMOUNT`) are "Not provided". AnyDash does not estimate remaining quota from usage.
- External costs are shown in the provider's original currency. Local estimated costs are shown in USD. There is no currency conversion. Estimated costs are labeled separately from subscription fees and actual bills.
- Plan quotas that a provider publishes only in its documentation, not through its API (`PLAN_DOCUMENTED_QUOTA`), show "Not provided".
- AnyDash never refreshes a CLI's stored OAuth token, because refreshing can invalidate the CLI's single-use refresh token. For providers that use local CLI credentials, the collector reads the credential again on every cycle. When the credential has expired, the connection shows "authentication expired" (`AUTH_EXPIRED`) and stops polling. Polling resumes automatically when a valid credential appears, for example after the user runs or signs in to that CLI again. Adapters never send a request to a token refresh endpoint.
- Credential reads are read-only: adapters never write, move or lock a CLI's credential files or keyring items.
- Unknown enum-like provider values (such as a new plan type) never break parsing; adapters report them as text and keep the remaining metrics.
- A response that cannot be parsed returns `FAILURE_CLASS_RESPONSE_UNREADABLE` for the affected metrics only; readable parts are still reported (`COLLECTION_STATUS_PARTIALLY_SUCCEEDED`).
- Provider rate limits take precedence over the AnyDash refresh cadence; adapters honor `Retry-After` and documented limits. HTTP 429 or 401 in any request of a cycle fails the whole cycle; other failed sub-requests leave the affected metrics `FAILED` and the snapshot `PARTIALLY_SUCCEEDED`.
- Decimal amounts are summed exactly in units of 10^-9 and reported without trailing zeros. Provider service names used as dimensions replace whitespace with `_`.

### Account fingerprints

`Connection.account_fingerprint` is the lowercase hex SHA-256 of `<Provider value name>:<account ID>`:

| Provider    | Account ID                                        |
| ----------- | ------------------------------------------------- |
| Codex       | `ChatGPT-Account-Id` from the Codex login         |
| Claude Code | Empty (no stable identifier is read)              |
| Grok        | Empty (AnyDash does not handle the Grok token)    |
| Vercel      | Team ID                                           |
| Supabase    | Empty (personal access tokens span organizations) |
| Cloudflare  | Empty (tokens can span accounts)                  |

The collector stores the connection's fingerprint when the connection is added or registered on the device (the server's value, or the device's own when the server has none). A local CLI login can change accounts after that, so before each active poll and passive report the Codex adapter compares the fingerprint of the current Codex login with the stored one. A different account, an API-key login or a login without an account ID is not reported under the connection: the provider is not called, the snapshot reports `COLLECTION_STATUS_AUTH_EXPIRED` with `FAILURE_CLASS_CREDENTIAL_MISSING`, the device stops listing the connection as ready and releases a held lease so a ready device of the connection's account can take over. Collection resumes when the Codex CLI is signed in to the connection's account again. A connection without a fingerprint accepts any login.

### Subscription usage

| Provider                                    | Metrics                                                                                                             | Source                                                                                                                                                       | Auth                                                                                                                                                                          | Notes                                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Codex (ChatGPT plans)                       | 5-hour and weekly used %, window length, reset time, plan type, credits                                             | Passive: `event_msg` `token_count.rate_limits` in local session JSONL. Active: `GET https://chatgpt.com/backend-api/wham/usage` every external polling cycle | Access token and `ChatGPT-Account-Id` from the Codex CLI's local login (`$CODEX_HOME/auth.json` or the OS keyring service `Codex Auth`), read only                            | The endpoint is undocumented but the official CLI calls it ([client.rs](https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/backend-client/src/client.rs), [protocol.rs](https://github.com/openai/codex/blob/67f0994b532b8740f14b5b738af7b7b2a915b1d6/codex-rs/protocol/src/protocol.rs)). Unknown `plan_type` values must not break parsing. API-key login has no subscription quota, so it shows "Not provided". |
| Claude Code (claude.ai plans)               | 5-hour, 7-day and per-model weekly utilization and reset times, extra usage                                         | `GET https://api.anthropic.com/api/oauth/usage` with `anthropic-beta: oauth-2025-04-20`                                                                      | Claude Code's locally stored claude.ai OAuth access token (macOS Keychain, or `~/.claude/.credentials.json`, which `CLAUDE_CONFIG_DIR` relocates), read only, never refreshed | Undocumented and rate-limited (HTTP 429). **Policy risk:** see [Known risks](#known-risks). If the endpoint becomes unavailable or Anthropic objects, AnyDash shows the failure state and the last successful value. No automatic fallback path exists. The response is a fix-forward release. Console API key, Bedrock, Vertex and Foundry logins have no subscription quota, so they show "Not provided".                                            |
| Grok (SuperGrok / X Premium via Grok Build) | Credit usage %, current period (weekly or monthly, start and end), on-demand cap and used amount, subscription tier | The official `grok` CLI's ACP extension method `x.ai/billing`, called through `grok agent stdio`                                                             | The `grok` CLI's own login. AnyDash does not handle the token.                                                                                                                | The method exists in official source ([billing.rs](https://github.com/xai-org/grok-build/blob/2bdd1d6a6369de0e8c68132ea4539e9abd9e14a8/crates/codegen/xai-grok-shell/src/extensions/billing.rs)) but is not publicly documented. Team principals are not supported, so they show "Not provided". Users without the `grok` CLI get "Not provided".                                                                                                      |

Metric mapping:

- Codex: the login is `$CODEX_HOME/auth.json`, otherwise the keyring item of service `Codex Auth` and account `cli|<first 16 hex digits of the SHA-256 of the canonical CODEX_HOME path>`; `tokens.access_token`, `tokens.account_id` and the access token JWT `exp` are read. In `wham/usage`, `rate_limit` is the limit with ID `codex` and each `additional_rate_limits` entry uses its `metered_feature` as limit ID. For the `codex` rate limit (the entry with limit ID `codex`, otherwise the first entry), the primary window maps to dimension `five_hour` and the secondary window to `weekly`: `used_percent` to `SUBSCRIPTION_USED_PERCENT`, `limit_window_seconds` (passive: `window_minutes`) to `SUBSCRIPTION_WINDOW_DURATION` and `reset_at` (passive: `resets_at`, Unix seconds) to `SUBSCRIPTION_RESET_AT`. `plan_type` maps to `SUBSCRIPTION_PLAN` as text, including unknown values. `credits.balance` maps to `SUBSCRIPTION_CREDITS_BALANCE` and `credits.unlimited` to `SUBSCRIPTION_CREDITS_UNLIMITED`. Additional rate limits use their limit ID as a dimension prefix (for example `<limit_id>.weekly`). `SUBSCRIPTION_REMAINING_AMOUNT` and `PLAN_DOCUMENTED_QUOTA` are `NOT_PROVIDED`. An API-key login (`auth.json` without ChatGPT tokens) reports `COLLECTION_STATUS_NOT_PROVIDED` with every subscription metric `NOT_PROVIDED`.
- Codex passive observations: the lease holder reads `rate_limits` from `token_count` events in its own local sessions and reports them with `OBSERVATION_SOURCE_PASSIVE_SESSION` and the event timestamp as `observed_at`, only when newer than its latest active observation and only under its valid lease. The active poll still runs every cycle.
- Claude Code: each top-level usage window object with `utilization` and `resets_at` maps to `SUBSCRIPTION_USED_PERCENT` and `SUBSCRIPTION_RESET_AT` with the object key as dimension (`five_hour`, `seven_day` and per-model weekly keys such as `seven_day_opus` or `seven_day_sonnet`). A `null` window is `NOT_PROVIDED`. `extra_usage` maps `is_enabled` to `SUBSCRIPTION_EXTRA_USAGE_ENABLED`, `utilization` to `SUBSCRIPTION_EXTRA_USAGE_USED_PERCENT`, and the used and monthly limit credits to `SUBSCRIPTION_EXTRA_USAGE_USED_AMOUNT` and `SUBSCRIPTION_EXTRA_USAGE_LIMIT_AMOUNT` in the currency the response reports (empty currency code when it reports none). Plan type is `NOT_PROVIDED` unless the response carries it (`plan_type` or `subscription_type`). `used_credits` and `monthly_limit` are minor currency units and are reported in major units (divided by 100). HTTP 401 means `AUTH_EXPIRED`; HTTP 429 follows `Retry-After`. The login is read from the macOS Keychain item `Claude Code-credentials` (with `-<first 8 hex digits of the SHA-256 of the first CLAUDE_CONFIG_DIR entry>` appended when `CLAUDE_CONFIG_DIR` is set) for the account `$USER`, otherwise from `.credentials.json` in the Claude Code configuration directories: `claudeAiOauth.accessToken` and `expiresAt` (Unix milliseconds). A credential without `claudeAiOauth` is a non-claude.ai login and reports `COLLECTION_STATUS_NOT_PROVIDED`.
- Grok: `creditUsagePercent` maps to `SUBSCRIPTION_USED_PERCENT` (dimension `credits`); `currentPeriod.type` maps to `SUBSCRIPTION_PERIOD_KIND` (`weekly` or `monthly`), `currentPeriod.start` to `SUBSCRIPTION_WINDOW_START` and `currentPeriod.end` to `SUBSCRIPTION_RESET_AT`; `onDemandCap` and `onDemandUsed` (USD cents) map to `SUBSCRIPTION_ON_DEMAND_CAP_AMOUNT` and `SUBSCRIPTION_ON_DEMAND_USED_AMOUNT` in `USD`; `subscriptionTier` maps to `SUBSCRIPTION_PLAN`. A missing `grok` executable reports `FAILURE_CLASS_CLI_UNAVAILABLE` with all metrics `NOT_PROVIDED`; an unauthenticated `grok` CLI reports `AUTH_EXPIRED`. The adapter starts `grok agent stdio`, completes the ACP initialization (`protocolVersion` 1), sends one `x.ai/billing` extension request (on the wire `_x.ai/billing`, because ACP custom methods carry an underscore prefix), and terminates the child process within the 30-second timeout. A JSON-RPC error with code -32000 or an authentication message means `AUTH_EXPIRED`. A result without `config` (for example a team principal) reports `COLLECTION_STATUS_NOT_PROVIDED` with every metric `NOT_PROVIDED`. `currentPeriod.type` values such as `USAGE_PERIOD_TYPE_WEEKLY` map to `weekly` or `monthly`; the tier comes from `subscription_tier` (or `subscriptionTier`). A Grok connection can be added without the `grok` executable.

### Local session usage

Local session usage covers Codex and Claude Code.

- The collector auto-detects the default paths and the official environment variable overrides. Codex: `$CODEX_HOME/sessions/YYYY/MM/DD/*.jsonl` (default `$CODEX_HOME` is `~/.codex`), compressed `.jsonl.zst` files, and `archived_sessions/`. Claude Code: `~/.claude/projects/**/*.jsonl`, `$XDG_CONFIG_HOME/claude`, and `CLAUDE_CONFIG_DIR`, which can hold comma-separated values. Users can add paths and disable detected paths.
- Metrics per device and in total: session count, input, output, cache-read and cache-write tokens, and estimated cost.
- Parsing rules:
  - Codex `total_token_usage` is cumulative per session. Compute each delta from the previous total. Codex `input_tokens` already includes cached tokens.
  - Claude Code entries are deduplicated by `message.id` + `requestId`. When duplicates exist, keep the entry with the larger token total.
  - Records that the parser cannot read are skipped. Readable records are still aggregated. Each device and tool shows a "partial aggregation" state with the count of skipped records and asks the user to update the collector.
- On first connection, the collector backfills the last 90 days of local sessions. Claude Code deletes transcripts after 30 days by default (`cleanupPeriodDays`), so backfill is limited to the files that still exist.
- When the same session files are copied to several devices, the per-device totals are still added together. There is no cross-device deduplication. The UI tells users about possible duplication.
- Original Codex and Claude Code session files on the device are never modified or deleted.
- The [CLI contract](cmds-anydash-cli-contract.md#session-parsing) defines the record fields, cursors and bucket assignment.

### Deployment platforms (read only)

| Provider   | Project list                                             | Status                                                                       | Usage                                                                                                                    | Limits                                                                                                | Cost                                                                                                            | Auth                                                                                                                                                        |
| ---------- | -------------------------------------------------------- | ---------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Vercel     | `GET /v10/projects`                                      | `live`, `paused`, `latestDeployments[].readyState` from the project response | `GET /v1/billing/charges` (FOCUS, daily granularity) `ConsumedQuantity` by service and project                           | Plan name from `GET /v2/teams/{teamId}`. Included quotas and the On-Demand Budget are "Not provided". | `BilledCost` / `EffectiveCost` in `BillingCurrency`, month to date                                              | Team-scoped token. The connection screen warns that the token can write, because Vercel has no read-only personal token. AnyDash calls only read endpoints. |
| Supabase   | `GET /v1/projects` and organization project lists        | Project `status` enum and `GET /v1/projects/{ref}/health`                    | `usage.api-counts`, `usage.api-requests-count`, `functions.combined-stats` (windows of 7 days or less), disk utilization | Organization `plan` and `entitlements` values. Documented-only quotas are "Not provided".             | "Not provided" (no spend API)                                                                                   | Fine-grained read-only personal access token                                                                                                                |
| Cloudflare | `GET /accounts`, then Workers scripts and Pages projects | Pages latest deployment stage status. Workers status is "Not provided".      | GraphQL Analytics: Workers invocations and CPU, R2, D1 and KV datasets                                                   | Plan from `GET /accounts/{id}/subscriptions`. Documented-only quotas are "Not provided".              | `GET /accounts/{id}/billable-usage` (beta, self-serve accounts, daily). Enterprise accounts get "Not provided". | API token with Account Analytics Read, Workers Scripts Read, Pages Read and Billing Read                                                                    |

Mapping rules:

- Requests per cycle: Vercel lists up to 10 pages of 100 projects; Supabase reads up to 50 projects and up to 20 functions per project; Cloudflare reads up to 20 accounts. Vercel billing charges cover the current UTC month to now; Supabase API counts use the `7day` interval and function statistics the `1day` interval; Cloudflare GraphQL Analytics covers the last 24 hours and billable usage the current month.
- Dimensions: Vercel totals use `total` and service dimensions; Supabase plans use the organization slug and entitlements `<organization slug>:<entitlement key>`, project usage uses `api_counts.<field>`, `api_requests_count` and `functions.<function slug>.<field>`; Cloudflare account-level dimensions are prefixed with the account ID (`<account ID>.<rate plan ID>`, `<account ID>.<dataset>.<field>`, `<account ID>.<service>` and `<account ID>` for the billed total), and Workers usage per script uses `workers.requests` and `workers.cpu_time`.
- Resources: Vercel projects (`RESOURCE_KIND_VERCEL_PROJECT`, parent team ID), Supabase projects (`RESOURCE_KIND_SUPABASE_PROJECT`, parent organization ID), Cloudflare Workers scripts and Pages projects (`RESOURCE_KIND_CLOUDFLARE_WORKER_SCRIPT`, `RESOURCE_KIND_CLOUDFLARE_PAGES_PROJECT`, parent account ID). Workers script names are unique only within an account and one token can span several accounts, so a Workers resource ID is `<account ID>/<script name>` with the script name as display name; Pages projects use the project ID. `provider_status` keeps the raw status. `ResourceHealth` normalization: ready, live, healthy and success states are `HEALTHY`; building, queued, initializing and restoring states are `IN_PROGRESS`; error, failure and canceled states are `FAILED`; unhealthy service health is `DEGRADED`; paused states are `PAUSED`; inactive states are `INACTIVE`; others are `UNKNOWN`. Cloudflare Workers status is `NOT_PROVIDED`.
- Usage: `DEPLOYMENT_USAGE_QUANTITY` with the provider service, metric or dataset name as dimension and the provider unit, `CUMULATIVE` over the reported period, per resource when the provider attributes it, otherwise at account level. Supabase disk utilization maps to `DEPLOYMENT_DISK_UTILIZATION`. Supabase function statistics use windows of 7 days or less.
- Limits: plan names map to `DEPLOYMENT_PLAN`; Supabase entitlements to `DEPLOYMENT_ENTITLEMENT` (one dimension per entitlement key); included quotas and Vercel's On-Demand Budget are `NOT_PROVIDED` (`PLAN_DOCUMENTED_QUOTA`, `DEPLOYMENT_ON_DEMAND_BUDGET`).
- Cost: Vercel `BilledCost` and `EffectiveCost` map to `DEPLOYMENT_BILLED_COST` and `DEPLOYMENT_EFFECTIVE_COST` in `BillingCurrency`, month to date, per service and project dimension and as totals. Cloudflare billable usage (`ContractedCost` in `BillingCurrency`) maps to `DEPLOYMENT_BILLED_COST`, month to date; enterprise accounts and accounts without access (HTTP 400, 403 or 404) are `NOT_PROVIDED`. Supabase cost is `NOT_PROVIDED`.
- Deployment history, incidents, logs and write operations (such as redeploys) are out of scope. Adapters call only the read endpoints above. The Vercel connection screen warns that the token can write.
- Cloudflare requests stay within the [Cloudflare API limits](https://developers.cloudflare.com/fundamentals/api/reference/limits/).

Provider references: [Vercel projects](https://vercel.com/docs/rest-api/projects/retrieve-a-list-of-projects), [Vercel FOCUS charges](https://vercel.com/docs/rest-api/billing/list-focus-billing-charges), [Vercel tokens](https://vercel.com/docs/accounts/access-tokens), [Supabase Management API](https://supabase.com/docs/reference/api/introduction), [Supabase PATs](https://supabase.com/docs/guides/platform/personal-access-tokens), [Cloudflare Workers scripts](https://developers.cloudflare.com/api/resources/workers/subresources/scripts/methods/list/), [Cloudflare Pages projects](https://developers.cloudflare.com/api/resources/pages/subresources/projects/methods/list/), [Cloudflare billable usage](https://blog.cloudflare.com/billable-usage-api/), [Cloudflare API limits](https://developers.cloudflare.com/fundamentals/api/reference/limits/).

### Failure classification

| Condition                                                      | `FailureClass`        | Snapshot status     | Next attempt                                 |
| -------------------------------------------------------------- | --------------------- | ------------------- | -------------------------------------------- |
| Timeout after 30 seconds                                       | `TIMEOUT`             | `FAILED`            | Backoff                                      |
| DNS, TLS or connection failure                                 | `NETWORK`             | `FAILED`            | Backoff                                      |
| HTTP 5xx or an unexpected status                               | `PROVIDER_ERROR`      | `FAILED`            | Backoff                                      |
| HTTP 429                                                       | `RATE_LIMITED`        | `RATE_LIMITED`      | `Retry-After`, otherwise backoff             |
| HTTP 401, expired local CLI credential, unauthenticated `grok` | `AUTH_EXPIRED`        | `AUTH_EXPIRED`      | Stop; resume when a valid credential appears |
| HTTP 403 for a missing token permission                        | `PERMISSION_DENIED`   | `FAILED`            | Backoff                                      |
| Unparsable response                                            | `RESPONSE_UNREADABLE` | `FAILED` or partial | Backoff                                      |
| Missing `grok` executable                                      | `CLI_UNAVAILABLE`     | `NOT_PROVIDED`      | Next regular cycle                           |
| Missing local CLI credential or stored token                   | `CREDENTIAL_MISSING`  | `AUTH_EXPIRED`      | Next regular cycle                           |

For official API tokens (Vercel, Supabase, Cloudflare), `AUTH_EXPIRED` stops the connection on the device until the user reconnects with a new token. The device stops listing the connection as ready and releases a held lease, so another registered device with a valid token can take over.

### Known risks

- **Claude Code policy risk.** Anthropic's [legal and compliance page](https://code.claude.com/docs/en/legal-and-compliance) states that "developers may not collect, store, or intermediate Claude.ai credentials or session tokens." The decided design reads the locally stored token on the user's own device to call `/api/oauth/usage`. Anthropic may consider this prohibited and may enforce without notice. The documented alternative is the statusline `rate_limits` input ([statusline docs](https://code.claude.com/docs/en/statusline)), which is out of scope by decision. An automatic fallback path (for example a statusline hook) is out of scope.
- **Undocumented endpoints.** The Codex `wham/usage` endpoint, Claude `/api/oauth/usage` and Grok `x.ai/billing` can change without notice. The parser and provider alerts are the detection path.
- **Provider coverage.** Plan coverage for ChatGPT Business and Enterprise, and for Claude Team and Enterprise, is unverified. Vercel billing access on Hobby plans is undocumented. Cloudflare billable usage is beta and self-serve only. Claude Code deletes transcripts after 30 days by default, so the 90-day backfill can be shorter.

### Out of scope

Unofficial collection through browser sessions or cookies; AnyDash refreshing CLI OAuth tokens; an automatic Claude Code fallback path; Supabase cost, including configured add-on prices; plan quotas that providers publish only in documentation; xAI Management API spend for API keys; deployment write operations, deployment history, incidents and logs.

## Storage

Adapters keep no state of their own beyond the collector's local store: backoff state, the latest observation per connection and the credential-change marker used to resume local CLI connections. Official tokens are stored encrypted by the collector; local CLI credentials are read in place on every cycle and never copied into AnyDash storage.

## Security

External credentials stay on the device and are never uploaded to AnyDash, logged, or included in snapshots or errors. Adapters send credentials only to the provider endpoints listed above, over HTTPS. Provider response bodies are parsed in memory and never stored or logged.

## Logging

Log the provider, connection ID, endpoint name (not full URL with query secrets), HTTP status, failure class, duration and next attempt time. Never log tokens, account IDs, response bodies or resource names beyond provider IDs needed for troubleshooting.

## Build and Test

Mocked-provider integration tests for every provider cover success, "Not provided" fields, transient failure with backoff (1 to 30 minutes, jitter, 30-second timeout), HTTP 429 with `Retry-After`, and authentication expiry. Local CLI credential providers also test automatic resume after a credential refresh by the CLI, with a check that AnyDash never sends a refresh-token request. Parser unit tests cover unknown `plan_type` values and `rate_limits` extraction. Mocked tests do not replace the release gates: a real-account connection for every provider and a real check of each "Not provided" metric, performed by the maintainer.

## Dependencies and Integrations

Provider endpoints listed above, the Codex, Claude Code and `grok` CLIs' local credential locations, and the collector runtime in the [CLI contract](cmds-anydash-cli-contract.md). Snapshot semantics follow the [protocol contract](protos-anydash-v1-contract.md); public provider connection guides in `apps/delino-docs` describe required tokens, permissions and provided and "Not provided" metrics.

## Change Triggers

Update this contract, the CLI contract, the protocol contract (for new metric kinds or dimensions) and the public provider guides together when a provider endpoint, auth method or metric changes. Provider endpoint changes ship as fix-forward collector releases. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Project](project-anydash.md)
- [Repository defaults](repository-defaults.md)
- [CLI and collector](cmds-anydash-cli-contract.md)
- [Protocol](protos-anydash-v1-contract.md)
- [Server](servers-anydash-api-contract.md)
