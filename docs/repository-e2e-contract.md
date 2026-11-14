# Repository End-to-End Testing Contract

## Scope

This contract owns browser end-to-end (e2e) tests for web apps under `apps/`. It applies to `delino-accounts`, `delino-site` and `delino-docs`. Unit tests, API server tests and native platform tests are out of scope.

## Framework

- Use the [`e2e`](https://www.npmjs.com/package/e2e) npm package (SDK, runner and CLI) with the `@e2e-dev/web` engine. The engine drives Chromium through a pinned `playwright-core`; do not add `playwright` or `@playwright/test`.
- Use the `ai` package (Vercel AI SDK) for the model behind `agent.*` steps.
- Pin `e2e`, `@e2e-dev/web` and `ai` to exact versions in each app's `devDependencies`. Keep the three apps on the same versions. `e2e` is pre-1.0, so review its changelog for config and API changes before an upgrade.
- The runtime is Node.js from root `.nvmrc`. `e2e` requires Node.js 24.8 or newer.

## Layout

Each app that has e2e tests owns these paths:

| Path                       | Purpose                                                         |
| -------------------------- | --------------------------------------------------------------- |
| `apps/<app>/e2e.config.ts` | Runner config: target, app command, test glob and agents.       |
| `apps/<app>/e2e/`          | Test files named `*.e2e.ts`.                                    |
| `apps/<app>/.e2e/`         | Ignored run output: report, results, logs and the replay cache. |

- `projectId` is the app's package name.
- The config and the tests are part of the app's TypeScript project. Include `e2e` and `e2e.config.ts` in the app `tsconfig.json`, so `typecheck` covers them.
- Every app exposes the suite as the `test:e2e` package script (`e2e run`). Root `pnpm test:e2e` runs `turbo run test:e2e` for every app.

## App under test

- The runner starts the app development server through `app.command` and stops it when the run ends. Name the server binary (`rsbuild` or `rspress`) as the executable, not a wrapper script.
- The target URL is `http://127.0.0.1:0`. The runner selects a free loopback port and passes it as `{port}` to `--port`, with `--host 127.0.0.1`. This is a documented exception to the fixed development ports in the [environment contract](repository-environment-contract.md#process-boundaries), so a suite can run while `pnpm dev` holds the fixed ports.
- The app command receives only `PATH`, `HOME` and temporary-directory variables from the host. Build-time variables such as `PUBLIC_API_ORIGIN` use their defaults unless the config passes them explicitly.
- Server output goes to `.e2e/logs/app.log`.
- The suites test the development server. They do not test the production build, Cloudflare Pages `_headers` or `_redirects`, or deployed origins.

## Tests

- One top-level `describe` per feature and one behavior per test, in arrange, act and assert order.
- Prefer deterministic locator steps (`screen`, `browser`, `expect`) for exact values and navigation.
- Tag every test that calls `agent` with `agent` (`{ tags: ["agent"] }`). Such tests need model access; runs without a model key exclude them with `--exclude-tag agent`.
- End every `agent.act` with a locator or engine assertion, so the replay cache can record it.
- Do not put secrets in test code. Declare them under `credentials` or `secrets` in the config.

## Model

- Agents use Vercel AI Gateway through `gateway()` from `ai`. The default agent model is `openai/gpt-6-luna-fast`.
- `gateway()` reads `AI_GATEWAY_API_KEY`, or a Vercel OIDC token (`VERCEL_OIDC_TOKEN`) when the key is not set. Locally, export the key in the shell that runs the suite. Never commit it.
- Tests without agent steps make no model calls and need no key.

## Replay cache

The replay cache is local state under `.e2e/cache/` and is not committed. Local runs record verified `agent.act` steps and replay them without model calls. CI starts with an empty cache in read-only mode, so agent steps in CI call the model on every run. Committing the cache requires a change to this contract and review of the recorded entries.

## Commands

Run from the repository root after `pnpm install --frozen-lockfile`:

- `pnpm exec turbo run test:e2e --filter=<app>`: run one app suite. Missing browsers download on first run.
- `pnpm test:e2e`: run every app suite.
- `pnpm test:e2e -- --exclude-tag agent`: run without agent-tagged tests, for example without a model key.
- `pnpm --filter <app> exec e2e-web install chromium`: install Chromium once before parallel runs, so several suites do not download it at the same time.

The root `test:e2e` Turbo task is uncached because results depend on a browser, a model and network access. It passes `AI_GATEWAY_API_KEY`, `VERCEL_OIDC_TOKEN`, `CI`, `E2E_*` and `PLAYWRIGHT_BROWSERS_PATH` through Turbo strict environment mode.

## CI

- The CI planner selects the `e2e` job when a path inside an `apps/<app>/` package changes, or when `.github/workflows/CI.yml`, `.nvmrc`, `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml` or `turbo.json` changes. The job runs every app suite on Ubuntu.
- The job installs Chromium with system dependencies and runs `pnpm test:e2e -- --reporter list,junit`.
- The model key comes from the `AI_GATEWAY_API_KEY` repository secret. Pull requests from forks do not receive secrets; when the key is empty, the job excludes tests tagged `agent` and records that in the job summary.
- The job uploads `apps/*/.e2e/` reports, results and logs as the `e2e-results` artifact for seven days.
- The job sets `E2E_TELEMETRY_DISABLED=1`.
- See the [workflow contract](repository-workflow-contract.md#validation) for the planner and the aggregate `ci-result` check.

## Security

- The model key reaches the tests, the config and every dependency in the job. Give it only to trusted branches; GitHub withholds secrets from fork pull requests.
- Set a spending limit on the AI Gateway key.
- Replay cache entries store typed values. Review them before any decision to share or commit them.

## Change Triggers

- Update this contract, the affected app contracts and the [workflow contract](repository-workflow-contract.md) when the framework, the layout, the model provider, the cache policy or the CI job changes.
- Add an app to this contract when it gains an e2e suite.

## References

- [e2e documentation](https://e2e.com/docs)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [repository-environment-contract.md](repository-environment-contract.md)
- [apps-delino-accounts-foundation.md](apps-delino-accounts-foundation.md)
- [apps-delino-site-foundation.md](apps-delino-site-foundation.md)
- [apps-delino-docs-foundation.md](apps-delino-docs-foundation.md)
