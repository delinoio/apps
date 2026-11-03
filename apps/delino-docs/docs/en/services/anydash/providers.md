# Provider connections

This page explains what each provider needs, which metrics AnyDash shows, and which metrics show "Not provided". Add connections in the desktop app (**Settings**, then **Connections**, then **Add connection**) or with [`anydash connect`](/services/anydash/cli#connect).

## How metrics work

- **Not provided:** the provider does not offer this value. The connection still works. "Not provided" is shown in grey text and is different from a temporary failure.
- **Couldn't refresh:** a collection attempt failed. AnyDash shows the last successful value, its time, and when it tries again.
- **Remaining:** AnyDash shows the remaining subscription allowance only as 100% minus the used percentage that the provider reports, for example "62% left". Absolute remaining amounts are "Not provided". AnyDash never estimates the remaining allowance from your usage.
- **Currency:** provider costs are shown in the currency that the provider reports. Estimated costs of local usage are shown in USD. AnyDash does not convert currencies. See [estimated cost](/services/anydash/data#estimated-cost).
- **Plan quotas:** quotas that a provider publishes only in its documentation, and not through its API, are "Not provided".
- **Read only:** AnyDash only reads data. It never changes your projects, deployments or settings, and it does not show deployment history, incidents or logs.

## Credentials stay on your device

Your API tokens and CLI sign-ins stay on the collector device. AnyDash sends a credential only to its own provider. It never uploads credentials to AnyDash and never writes them to logs. API tokens are stored encrypted; see [credential storage](/services/anydash/cli#credential-storage-and-encryption-keys).

### CLI sign-ins

Codex, Claude Code and Grok use the sign-in of their own CLI on the device. AnyDash reads the sign-in again on every cycle. **AnyDash never refreshes a CLI sign-in**, because a refresh by AnyDash could sign the CLI out. When the sign-in expires, the connection shows "Authentication expired" and stops polling. Sign in to that CLI again, or run it so that it renews its own sign-in, and AnyDash resumes automatically within 5 minutes. You do not need to reconnect.

### API tokens

Vercel, Supabase and Cloudflare use an API token that you create. When a token expires or is revoked, the connection shows "Authentication expired" and stops polling. Add the connection again with a new token.

## Refresh behavior

- External connections refresh every 5 minutes while a collector runs and is online.
- You can refresh a connection manually once per minute. Before then, the Refresh control shows "Refresh available in {n}s".
- Provider rate limits come first. When a provider asks AnyDash to wait, AnyDash shows "{Provider} asked us to wait" and the time of the next try.
- After a temporary failure, AnyDash tries again after 1 minute and doubles the wait after each further failure, up to 30 minutes, with a small random delay. It keeps trying. A request that takes longer than 30 seconds fails. The error state shows from the first failure, and a success resets the wait.

### Collection devices

You can register several of your devices for one connection. Each device stores its own credential, and only one device polls at a time. Each device reports to AnyDash every minute. If the current collection device does not report for 5 minutes, another registered device takes over: your preferred device first (set it in **Settings**, then **Devices**), otherwise the device that reported most recently. Results from a device that lost its turn are not accepted, even after it comes back. If no registered device is running, the connection shows "No collector running" and its metrics stop refreshing.

To register another device for an existing connection, select **Use on this computer** in the desktop app, or run `anydash connect <provider> --connection-id ID` on that device.

## Codex

**Requires:** the Codex CLI on the device, signed in with a ChatGPT account. AnyDash reads the Codex sign-in from `$CODEX_HOME/auth.json` or the OS keyring. You do not enter a token.

**Provided metrics:**

- 5-hour and weekly used percentage and remaining percentage.
- Window length and reset time for each window.
- Plan type. New plan types are shown as reported.
- Credits balance, and whether credits are unlimited.
- Additional limits that ChatGPT reports for specific features.

AnyDash polls the usage every 5 minutes. On the current collection device, it also uses the limit information recorded in recent Codex session files, when that information is newer.

**Not provided:**

- Absolute remaining amounts.
- Plan quotas that OpenAI publishes only in its documentation.
- All subscription metrics when Codex is signed in with an API key, because API-key sign-in has no subscription limits.

**Note:** the usage source is not a publicly documented API. It can change without notice. If it changes, AnyDash shows "Couldn't refresh" with the last successful value until a collector update supports the change.

## Claude Code

**Requires:** Claude Code on the device, signed in with a claude.ai account. AnyDash reads the Claude Code sign-in from the macOS Keychain, or from `.credentials.json` in the Claude Code configuration folder (`~/.claude`, or the folder that `CLAUDE_CONFIG_DIR` sets). You do not enter a token.

**Provided metrics:**

- 5-hour, 7-day and per-model weekly used percentage and remaining percentage.
- Reset time for each window.
- Extra usage: whether it is on, its used percentage, the used amount and the monthly limit, in the currency that Anthropic reports.
- Plan type, when Anthropic reports it.

**Not provided:**

- Absolute remaining amounts.
- A usage window that Anthropic does not report for your plan.
- Plan type, when Anthropic does not report it.
- All subscription metrics for Claude Console API keys and for Amazon Bedrock, Google Vertex AI and Microsoft Foundry sign-ins, because these have no subscription limits.

**Note:** the usage source is not a publicly documented API and is rate-limited. If it becomes unavailable, AnyDash shows "Couldn't refresh" and the last successful value. AnyDash has no alternative source for these metrics.

## Grok

**Requires:** the official `grok` CLI on the device, signed in with a SuperGrok or X Premium subscription through Grok Build. AnyDash asks the `grok` CLI for its billing information. AnyDash never handles the Grok token.

**Provided metrics:**

- Credit usage percentage and remaining percentage.
- Current period: weekly or monthly, with its start and end.
- On-demand cap and on-demand used amount, in USD.
- Subscription tier.

**Not provided:**

- All metrics when the `grok` CLI is not installed. You can add the connection before you install the CLI.
- All metrics for team accounts.
- Absolute remaining amounts and plan quotas that xAI publishes only in its documentation.

When the `grok` CLI is not signed in, the connection shows "Authentication expired". Sign in to `grok` again, and AnyDash resumes at its next cycle.

**Note:** the billing information is not a publicly documented API. It can change without notice.

## Vercel

**Requires:** a team-scoped Vercel access token and the team ID.

1. Open the [Account Tokens page](https://vercel.com/account/tokens) in your Vercel account settings.
2. Enter a name. Under **Scope**, select the team, and then select **All Projects**. A project-scoped token cannot read the team's plan and billing.
3. Choose an expiration and create the token. Copy it; Vercel shows it only once.
4. Find the team ID in the team settings.

To read billing charges, your role in the team must allow billing access in Vercel (Owner, Member, Developer, Security, Billing or Enterprise Viewer).

> **Warning:** Vercel has no read-only token. The token can also make changes to the team. AnyDash calls only read endpoints, and the token stays on your device. The connection screen shows this warning.

**Provided metrics:**

- Projects, with their live or paused state and the state of their latest deployments.
- Usage by service and project for the current month, from Vercel billing charges (daily granularity).
- Billed cost and effective cost for the current month, by service and project and in total, in the billing currency.
- Plan name.

**Not provided:**

- Included plan quotas.
- On-Demand Budget.

## Supabase

**Requires:** a Supabase personal access token with read-only permissions.

1. Open [Access Tokens](https://supabase.com/dashboard/account/tokens) in your Supabase account settings and generate a new token.
2. Select the organizations and projects that AnyDash can read.
3. Grant **Read** access, not Read-write, to these permissions: Organizations, Organization Settings, Organization Projects, Projects (account-wide), Project Settings, Usage Analytics, Edge Functions and Disk Config.

If a permission is missing, the affected metrics show "Couldn't refresh" with a permission error, and the other metrics still refresh.

**Provided metrics:**

- Projects, with their status and service health. A project with an unhealthy service shows as degraded.
- API request counts by service (Auth, Realtime, REST and Storage) for the last 7 days, and the API request count.
- Edge Function statistics for the last day.
- Disk utilization.
- Organization plan and entitlements.

AnyDash reads up to 50 projects and up to 20 Edge Functions for each project.

**Not provided:**

- Cost. Supabase has no spend API, so AnyDash does not show Supabase costs or add-on prices.
- Plan quotas that Supabase publishes only in its documentation.

## Cloudflare

**Requires:** a Cloudflare API token with these account permissions:

- Account Analytics Read
- Workers Scripts Read
- Cloudflare Pages Read
- Billing Read

Create a custom token in the Cloudflare dashboard under **My Profile**, then **API Tokens**. Grant only the Read permissions above, for the accounts that AnyDash can read.

**Provided metrics:**

- Workers scripts and Pages projects for each account. Pages projects show the status of their latest deployment.
- Usage for the last 24 hours from Cloudflare analytics: Workers requests and CPU time for each script, and R2, D1 and KV usage.
- Plan for each account.
- Billed cost for the current month for self-serve accounts, in the billing currency. Cloudflare provides this data in beta, with daily granularity.

AnyDash reads up to 20 accounts.

**Not provided:**

- Workers status.
- Cost for Enterprise accounts, and for accounts where the token cannot read billing.
- Plan quotas that Cloudflare publishes only in its documentation.

## Local session usage

Codex and Claude Code local session usage needs no connection. The collector reads the session files on each device and shows, for each device and in total:

- Session count.
- Input, output, cache-read and cache-write tokens.
- [Estimated cost](/services/anydash/data#estimated-cost) in USD.

The collector reads the default folders and the folders that `CODEX_HOME` and `CLAUDE_CONFIG_DIR` set. You can add and disable folders with [`anydash paths`](/services/anydash/cli#paths) or in **Settings**, then **Session paths**. AnyDash counts each session once, in the 15-minute period when it started, and counts tokens in the period when they were used.

If the collector cannot read some records, for example after a format change in Codex or Claude Code, it skips them and still counts the readable records. The device and tool then show "Partial aggregation · {n} records skipped". Update the collector to read them.

If you copy the same session files to more than one device, AnyDash counts their usage once for each device. See [cross-device duplication](/services/anydash/data#cross-device-duplication).
