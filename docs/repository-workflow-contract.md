# Repository workflow contract

Generated protobuf and Connect bindings under `protos/` (`*.pb.go`, `*.connect.go` and `*_pb.ts`) are excluded from tokei counts through `.tokeignore` and from GitHub language statistics through `.gitattributes`. Schemas and hand-written files remain eligible for counting.

Root `pnpm dev` runs every workspace `dev` script concurrently through Turborepo. The `dev` task is persistent and uncached. The current participants are DeliDev, DeliTools, DevBase, Delino Site, Delino Docs, FixKey and Git Tracker frontends, plus the Delcore and Git Tracker Go APIs. Each retains its documented fixed loopback port and fails on conflicts. Stop the development session with `Ctrl+C`. App-local `pnpm dev` and `pnpm dev:desktop` remain available for individual frontend and native development. Each Go API also supports service-local `pnpm dev`. The server scripts run pinned Air `v1.67.4` through `go run`, without a global Air install. Each service configures Air with `root = "../.."` to watch shared files from the repository root. Build and run commands remain relative to the service workspace; no shell directory change is needed. Air rebuilds and restarts the owning server for Go or SQL changes in its service or `protos/`, and for root `go.mod` or `go.sum` changes. Each service uses a separate ignored `tmp/` directory and loads its optional service-local `.env`. Turborepo passes `CONFIG` to Delcore, the Git Tracker server's unprefixed runtime settings and `ALERT_*` to Git Tracker, and `OTEL_*` and `NO_COLOR` to both API development tasks. Configure the existing database and provider requirements before starting; development does not provision databases, apply migrations or supply credentials. See the [Delcore server contract](servers-delcore-api-contract.md#local-development) and [Git Tracker server contract](servers-git-tracker-api-contract.md#local-development) for configuration. Each app-local `dev` and `dev:*` command and each Go API `dev` command injects the linked Infisical project's `dev` variables before startup. Install and authenticate the CLI and initialize the local root project configuration as described in the [environment contract](repository-environment-contract.md#local-development-injection). Root development delegates injection to each workspace task.

## Local development databases

Root `compose.yaml` provides separate PostgreSQL 17 instances for Delcore and Git Tracker, matching the major version used by server integration CI. The frontends, native shells and Air-managed APIs run on the host through the existing development commands. Install Docker Engine and Docker Compose v2.20 or later, then start Docker. Run these commands from the repository root:

```sh
docker compose up -d --wait
docker compose ps
```

`--wait` waits for both database healthchecks. To start only one database, append `delcore-postgres` or `git-tracker-postgres` to the `up` command. Both databases publish only on `127.0.0.1`, use separate named data volumes and fail on port conflicts. The committed usernames and passwords are disposable local development values. Never use them for shared or production deployments.

- Delcore: `postgres://delcore:local-delcore@127.0.0.1:5432/delcore?sslmode=disable`
- Git Tracker: `postgres://git_tracker:local-git-tracker@127.0.0.1:55432/git_tracker?sslmode=disable`

Set Delcore's `database_url` in its operator-owned JSON configuration to the first URL. Set Git Tracker's `DATABASE_URL` in its development configuration to the second URL. Keep the remaining provider and authentication configuration under the owning [Delcore](servers-delcore-api-contract.md#local-development) and [Git Tracker](servers-git-tracker-api-contract.md#local-development) contracts. Compose does not provision Logto, Paddle, GitHub or email provider accounts. Configure Infisical as described in the [environment contract](repository-environment-contract.md#local-development-injection); injected settings must point to these local databases when using this workflow.

Apply migrations explicitly before the first API start and after schema changes. From the repository root, with `CONFIG` set to the absolute path of the local Delcore JSON configuration, run:

```sh
go run ./servers/delcore-api/cmd/delcore-api -config "$CONFIG" -migrate
DATABASE_URL='postgres://git_tracker:local-git-tracker@127.0.0.1:55432/git_tracker?sslmode=disable' go run ./servers/git-tracker-api/cmd/git-tracker-api migrate
pnpm dev
```

Database health does not establish schema readiness. Compose does not apply application migrations. Root `pnpm dev:portless` and service-local `pnpm dev` can also use these databases.

Inspect logs with `docker compose logs -f delcore-postgres git-tracker-postgres`. Open a SQL session with either command:

```sh
docker compose exec delcore-postgres psql -U delcore -d delcore
docker compose exec git-tracker-postgres psql -U git_tracker -d git_tracker
```

Stop and remove containers with `docker compose down`; the data volumes remain. `docker compose down --volumes` also deletes both local databases. After deleting volumes, start Compose and reapply the migrations.

For another checkout or occupied database ports, select an explicit project name and host ports on every Compose command. For example:

```sh
DELCORE_POSTGRES_PORT=15432 GIT_TRACKER_POSTGRES_PORT=15433 docker compose -p apps-local-second up -d --wait
DELCORE_POSTGRES_PORT=15432 GIT_TRACKER_POSTGRES_PORT=15433 docker compose -p apps-local-second down
```

Update the database URLs and migration commands to the selected host ports. Each Compose project has its own data volumes. These overrides affect database ports only; application listeners retain their fixed ports, so concurrent worktrees cannot serve the same application on the host. PostgreSQL integration tests require separate disposable test databases through `TEST_DATABASE_URL`; do not point tests at databases containing development data.

See the Docker references for [Compose startup and healthchecks](https://docs.docker.com/compose/how-tos/startup-order/) and [PostgreSQL image storage and initialization](https://hub.docker.com/_/postgres).

## Named local development URLs

The root pins Portless as a development dependency. Run `pnpm dev:portless` from the repository root to start all seven frontends and both Go APIs through the existing Turbo `dev` tasks and register their local URLs. Root `portless.json` owns app names and upstream ports, including the Rspress documentation server and Air-managed APIs. Portless does not change the servers' listeners: keep Delcore configured on `127.0.0.1:8080` and Git Tracker on `127.0.0.1:46321`. Prepare each API's database, schema and provider configuration before starting, as for root `pnpm dev`. Each task retains its Infisical injection and the API environment allowlists. Run Portless from the root so it reads this configuration. App-local `pnpm dev` and native development commands retain their direct loopback addresses.

| Application     | Main-checkout URL                   | Upstream          |
| --------------- | ----------------------------------- | ----------------- |
| DeliDev         | `https://delidev.localhost`         | `127.0.0.1:46311` |
| DeliTools       | `https://delitools.localhost`       | `127.0.0.1:46312` |
| DevBase         | `https://devbase.localhost`         | `127.0.0.1:46313` |
| Delino Site     | `https://delino-site.localhost`     | `127.0.0.1:46314` |
| Delino Docs     | `https://delino-docs.localhost`     | `127.0.0.1:46315` |
| FixKey          | `https://fixkey.localhost`          | `127.0.0.1:46316` |
| Git Tracker     | `https://git-tracker.localhost`     | `127.0.0.1:46320` |
| Delcore API     | `https://delcore-api.localhost`     | `127.0.0.1:8080`  |
| Git Tracker API | `https://git-tracker-api.localhost` | `127.0.0.1:46321` |

Portless prefixes names with the branch in linked Git worktrees. Use the URLs printed at startup. Upstream ports remain fixed to preserve native shell development URLs and strict conflict handling; different worktrees cannot serve the same app concurrently on those ports.

Before the first session, run `pnpm exec portless proxy start` from an interactive terminal. Portless defaults to HTTPS on port 443, creates a local certificate authority, and can request administrator access to trust it and bind the port. This setup is local to the machine and is not an install hook. Non-interactive sessions must use an already prepared proxy. For HTTP without certificate setup or a privileged port, run `pnpm exec portless proxy start --no-tls --port 1355`; subsequent app commands reuse that proxy configuration and print URLs such as `http://devbase.localhost:1355`.

`Ctrl+C` stops the app session and removes its routes. The shared proxy remains available for other sessions. Inspect it with `pnpm exec portless list` or `pnpm exec portless doctor`; stop it with `pnpm exec portless proxy stop` when no session needs it. Portless keeps machine state outside the repository and can synchronize local host entries for browsers that need them. See the [upstream usage guide](https://github.com/vercel-labs/portless#readme) for trust and DNS troubleshooting. Go APIs retain their separate startup commands, origins and authentication configuration.

### Git Tracker API configuration

The Git Tracker frontend also needs an API URL with the proxy's scheme and an exact matching allowed web origin. The direct-loopback values in the app and server `.env.example` files apply to direct development, not the Portless URLs. An HTTPS page cannot call the HTTP loopback API, and an HTTP Portless page still needs its own allowed origin.

Root `pnpm dev:portless` starts the Git Tracker API through its Air task and registers its route automatically. For the default HTTPS proxy in the main checkout, set `PUBLIC_GIT_TRACKER_API_URL=https://git-tracker-api.localhost` in the frontend's Infisical development configuration, or in the uncommitted `apps/git-tracker/.env.local` when the injected environment does not define it. Set `WEB_ORIGIN=https://git-tracker.localhost` in the API's Infisical development configuration, process environment or ignored service-local `.env`. Keep its other required settings under the [server contract](servers-git-tracker-api-contract.md). Restart the frontend and API after changing their settings. Portless does not provision databases or supply credentials.

In a linked worktree, both web and API routes receive the branch prefix. Set `PUBLIC_GIT_TRACKER_API_URL` to the printed Git Tracker API origin and `WEB_ORIGIN` to the exact printed frontend origin. With the HTTP proxy on port 1355, the main-checkout API URL is `http://git-tracker-api.localhost:1355`; a linked worktree also includes its branch prefix. Include the scheme and any non-default proxy port, with no path or trailing slash. Register the frontend's `/callback` URL and sign-out origin in the development Logto application for the selected origin; authentication still requires its normal public application configuration.

When running the API separately without a root Portless session, an optional static route remains available after starting the proxy:

```sh
pnpm exec portless alias git-tracker-api 46321
```

This explicit alias has no automatic worktree prefix and persists until removed with `pnpm exec portless alias --remove git-tracker-api`. Remove an existing static alias before starting root `pnpm dev:portless`, which manages its own routes and removes them on shutdown. Point the frontend at the static alias only while using that separate API workflow. Fixed upstream ports still permit only one local instance. Restore direct-loopback API and allowed-origin settings before returning to direct frontend development.

The local Delcore route supports server and command-line clients. The current Delcore handler does not provide CORS preflight responses or allow-origin headers, so browsers cannot make authenticated Connect calls to it from a different frontend origin. Do not set `PUBLIC_DELCORE_API_URL` to the local Portless origin for this workflow. Browser access to local Delcore requires separate CORS support; a registered Logto origin alone does not enable it. Keep `PUBLIC_DELCORE_RESOURCE` set to the resource identifier registered with Logto.

For server-to-server calls from Git Tracker to local Delcore, set `DELCORE_URL` to the printed Delcore API origin and retain its registered `DELCORE_RESOURCE`. Portless provides transport routing; it does not change resource identifiers or register development origins with identity providers. Follow the [Delcore server contract](servers-delcore-api-contract.md#local-development) for its remaining settings.

## Validation

The root `package.json` pins pnpm 12.14.0; the shared CI Node setup uses the same version and a version-specific store cache. Dependency overrides and build-script decisions are configured in `pnpm-workspace.yaml`, because pnpm 12 does not read the `pnpm` field in `package.json`.

Install with `pnpm install --frozen-lockfile`. Run `pnpm test` in each app, root `cargo test`, and `go test ./...`. Generate frontend `dist` before Rust validation, then remove generated `dist` directories. Native builds, device launches, account-backed integration and store acceptance are separate checks. Preserve source revision, commands, results and unperformed checks in commits and CI artifacts.

CI compares PRs with their base, pushes with the previous revision, and manual runs with the main merge base (or the parent on main). Manual validation remains affected-only. Ordinary CI runs on Ubuntu; macOS is restricted to the separate main-only mobile release workflow and the FixKey release workflow for tags verified to belong to main. Protocol validation includes Go vet, unit tests and consumer example build; the dedicated formatting job checks Go formatting. Real PostgreSQL integration and OCI checks run for the affected services under `servers/`. Protocol changes require lint, format, regeneration freshness and FILE breaking checks against the comparison revision.

Mobile dispatch preserves signed build-only mode and opt-in Google Play internal/TestFlight uploads. Configure the protected environment described in the application contract before use. Release workflows do not establish store acceptance until run with real accounts.

The Tauri wrapper consumes the immutable release and hashes in `prebuilt-dependencies.lock.json`. These shared binaries remain published by `delinoio/oss`; keep that producer URL and source pin unchanged. Local source builds are explicit; CI source fallback is forbidden.

Use root `pnpm format` and `pnpm format:check` for changed supported files. Lefthook formats staged files while preserving unstaged changes. CI passes the exact comparison through `TURBO_SCM_BASE` and `TURBO_SCM_HEAD`. Track assets of at least 512 KiB with exact-path Git LFS entries and hydrate them before builds. Schema-only baseline checks skip unrelated LFS smudging.

Delino Site and Delino Docs run offline translation coverage and source-fingerprint checks in their owning `pnpm test` and `pnpm build` commands. Root `pnpm i18n:check` checks all workspaces that provide that script; root `pnpm i18n:translate` generates translations sequentially for all participating workspaces. Root account commands share the same local ChatGPT session as app-local commands. Shared localization fixtures participate in root `pnpm test:tooling`. Translation generation and ChatGPT authentication are local maintainer operations under the [localization contract](apps-delino-site-localization-contract.md); CI and static deployment require no ChatGPT credentials.

Older baselines use the former Go module path or the separate generated-code directory. Breaking validation normalizes only the exact historical `go_package` options to the `github.com/delinoio/apps` colocated package path, preserving all wire and remaining FILE checks. Remove this compatibility projection when all supported baselines use the colocated package path.

CI retains separate `devbase-frontend`, `delino-site-frontend`, `delino-docs-frontend`, `delidev-frontend`, `delitools-frontend`, `fixkey-frontend`, `fixkey-desktop`, `rust-clippy`, `rust-test`, `protocol`, `delcore-api`, and `git-tracker-api` jobs. The planner gates each project job before runner allocation. `check-formatting` (display name `Check Formatting`) owns all CI formatting checks: Rstack for the exact planner comparison, workspace Rust formatting when Rust packages are selected, and Go and Protobuf formatting when protocol validation is selected. This job runs on Ubuntu and is required by `ci-result`. `validate` checks tooling fixtures, shared UI type/interaction/CSS tests and workflow syntax. Changes in `packages/ui` select all frontend consumers through the shared-path planner. `ci-result` fails if any selected job fails, is canceled, or is skipped. Unselected project jobs must remain skipped.

The changes job runs pinned cargo-mono twice against the planner base: `changed --direct-only` selects Clippy packages, and `changed` expands reverse workspace dependencies for Test. Each package list gates its own Rust job before runner allocation; empty lists skip the job. Workspace manifests and lockfiles retain cargo-mono global-impact handling. CI tooling, Cargo configuration, toolchain and shared prebuilt dependency changes select all Rust packages. If cargo-mono resolves a different merge base from the event comparison, both checks conservatively select every workspace package. Frontend-only changes outside native crate directories do not select Rust checks.

Rust validation installs the Linux native dependencies, generates only the selected shells' frontend output, including FixKey when selected, runs all-target tests and all-target/all-feature Clippy with warnings denied. Rust formatting runs in `check-formatting`. Cache restores are available to PRs; pnpm and Cargo caches are saved only after successful main validation. Each validation job retains a JSON artifact with the source revision, comparison, commands, outcomes, and unperformed native/account checks. Protocol freshness rejects both tracked and untracked generated changes.

Server runtime validation runs in this checkout. `delcore-api` and `git-tracker-api` jobs use Ubuntu runners and the root Go module. Server and protocol changes select the affected service checks before runner allocation. Changes to `docs/protos-delcore-v1-contract.md` select Delcore server validation; changes to `docs/protos-git-tracker-v1-contract.md` select Git Tracker server validation. Each service runs Go vet/build/race tests and real PostgreSQL integration with mocked providers; Delcore also runs OCI tests. Go formatting remains in `check-formatting`. `ci-result` rejects failed, canceled or skipped selected server jobs. Record source revision, comparison, commands, results and unperformed live-account checks in the same review and CI evidence. See the [repository boundary contract](repository-boundary-contract.md).

## Turborepo remote caching

Use Vercel Remote Cache for cached Turbo builds. Configure the repository Actions variable `TURBO_TEAM` and secret `TURBO_TOKEN` under the [environment contract](repository-environment-contract.md#turborepo-remote-cache). CI runs the frontend preparation builds in `fixkey-desktop`, `rust-clippy` and `rust-test` through `pnpm exec turbo run build:frontend --filter=<workspace>`. The existing affected-job and package gates remain in place. Frontend test commands and native Rust validation still execute normally; they are not cached by this integration.

Main pushes can read and write remote artifacts. Affected manual runs on main use remote reads only. PRs, including same-repository PRs, and non-main manual runs receive no remote cache token and use `TURBO_CACHE=local:rw`. The token itself grants write access; Turbo read-only options do not reduce its authorization and cannot protect it from PR-controlled commands. If either credential setting is absent, trusted main jobs also use local caching. This policy does not change pnpm or Cargo cache handling. Turbo uploads each successful build task independently of later job results. Credentials and cached logs must never contain account data or signing material.

Build tasks restore `dist/**` and include ignored `.env*` files in their inputs. FixKey extends the root frontend inputs with `$TURBO_EXTENDS$` before adding its package-specific root files, so the environment-file inputs remain inherited. Shared UI source, localization tooling, root license notices, the Node selector and pnpm workspace configuration participate in the global hash alongside the existing Go and protocol inputs. `NODE_ENV`, `PUBLIC_*`, `RUNNER_OS` and `RUNNER_ARCH` values also participate in the hash. CI artifacts are therefore isolated by runner OS and architecture; local runs without those runner variables use separate entries. Keep additional build-affecting environment variables and external files in the owning Turbo inputs before using them in cached builds. FixKey's native `build` task is uncached because its native artifacts are outside the frontend output contract. Development and test tasks remain uncached.

For local builds, run `pnpm exec turbo login`, then `pnpm exec turbo link` from the repository root and select the same Vercel team. Local authentication and `.turbo/config.json` remain uncommitted. Run `pnpm exec turbo run build:frontend --filter=<workspace>` to cache a frontend preparation build, or root `pnpm build` for the existing workspace build tasks. Remove the generated `dist` directories after validation. To bypass remote access, set `TURBO_CACHE=local:rw`; use `--force` to execute a task despite a cache hit. Login and link require interactive Vercel account access.

See the [upstream GitHub Actions guide](https://turborepo.dev/docs/guides/ci-vendors/github-actions) and [cache controls](https://turborepo.dev/docs/reference/run#--cache-options).

## FixKey validation and release

FixKey engine tests and native-adapter Clippy participate in `rust-clippy` and `rust-test` on affected events. FixKey uses the central affected planner for `fixkey-frontend` and main-push-only `fixkey-desktop`. Frontend and release-helper fixtures run on Linux; desktop feature tests and clippy run on Linux and Windows without native application interaction. PRs and affected manual validation preserve native-job exclusions. Mac runners belong only to the separate tag-triggered release workflow after main-ancestry validation. The owning [desktop contract](apps-fixkey-desktop-contract.md#release-assembly) defines protected signing/publication environments, immutable release tags, complete signed updater artifacts and license assembly. Builds, fixtures and signature checks must not be reported as native application or installer acceptance.

## Project release preparation

`.github/workflows/release-project.yml` follows the shared preparation and project-specific publication pattern. Dispatch it from `main`, select `devbase`, `delino`, `delitools` or `fixkey`, and choose `patch`, `minor` or `major`. Only the selected release's application versions change. The `delino` release calculates the next version from `apps/delino-site/package.json` and writes it to both that manifest and `apps/delino-docs/package.json`, regardless of their previous version agreement. Other web-only releases update their own npm manifest. DeliTools and FixKey also update their Tauri configuration, native package manifest and corresponding workspace `Cargo.lock` entry. The FixKey engine version is independent and is not bumped with the desktop application. Native source versions must agree before preparation.

`scripts/release/project.mjs` owns version calculation, source validation and the Git transaction. Preparation uses the release bot's identity, commits the selected files and atomically pushes `main` and its immutable tag. A concurrent main change or an existing tag rejects publication without moving the remote branch or tag. Preparation is serialized across projects. The commit records the workflow run ID and bump; rerunning the same run validates and reuses its recorded source even if main has advanced. Changed inputs, source contents or tags are rejected. Start a new run for a new version. Do not replace a tag or bypass branch protection to recover a failed run.

Tags are `devbase@vMAJOR.MINOR.PATCH`, `delino@vMAJOR.MINOR.PATCH`, `delitools@vMAJOR.MINOR.PATCH` and `fixkey@vMAJOR.MINOR.PATCH`. A GitHub App installation for this repository supplies `DELINO_RELEASE_BOT_CLIENT_ID` as an Actions variable and `DELINO_RELEASE_BOT_PRIVATE_KEY` as a secret. It requires contents write access and repository rules that permit its release commits and tag creation. The app token allows tag pushes to start the owning workflows; the default Actions token does not trigger that follow-up automation. Checkout temporarily retains the app credential for the atomic push and removes it during post-job cleanup. No registry package publication is part of this apps release procedure.

DeliDev has no release entry because its owning desktop contract removes distribution automation. Delcore and Git Tracker server runtimes live under `servers/`; this release workflow does not deploy or publish server images.

## Web publication

The individual `.github/workflows/release-devbase.yml`, `release-delino.yml` and `release-delitools.yml` workflows publish the corresponding web applications. Each starts from its release tag. `release-delino.yml` publishes `delino-site` and `delino-docs` in parallel from the same `delino@vMAJOR.MINOR.PATCH` tag and source revision. Source validation checks both package versions. The former `delino-site@v*` trigger is retired. To retry a deployment without another bump, dispatch the individual workflow from `main` with the existing tag. A source job checks the tag format, version and main ancestry using the trusted main helper before allocating the protected deployment job. Each deployment job checks out the resolved immutable source revision, hydrates LFS objects, installs the frozen workspace and runs the app's `pnpm test` to check types and build the static output.

The Delino deployment matrix keeps `fail-fast: false` so one app failure does not cancel the other. The two Cloudflare uploads are not atomic; inspect both jobs and retry the existing tag if either fails.

The Delino Wrangler action sets `pnpm_config_ignore_workspace_root_check=true` only for its deployment step because the pinned action installs its CLI with `pnpm add` at the workspace root without `--workspace-root`. pnpm 12 requires the `pnpm_config_` prefix for this setting. This exception allows that temporary CLI installation without changing the workspace-wide pnpm configuration. Remove it when the action supports explicit workspace-root installation. The workspace build-script allowlist permits `workerd`, which Wrangler installs during deployment.

DevBase and DeliTools production deployments use the corresponding `<project>-production` GitHub environment with secret `CLOUDFLARE_API_TOKEN` and variables `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_PAGES_PROJECT`. Delino Site and Delino Docs share the `delino-site-production` environment with secret `CLOUDFLARE_API_TOKEN` and variable `CLOUDFLARE_ACCOUNT_ID`. Its `CLOUDFLARE_PAGES_PROJECT_SITE` and `CLOUDFLARE_PAGES_PROJECT_DOCS` variables select the marketing and documentation Pages projects, respectively. The deployment matrix maps each application to its own variable for both configuration validation and upload. The token needs Cloudflare Pages edit permission for the owning account. Configure reviewers and deployment branch/tag restrictions to permit the owning project tags and main-dispatched recovery. `delino-site-production` must permit `delino@v*` tags and main-dispatched recovery. Create each Pages project with production branch `main` and bind its documented domain before publication. Disable automatic Git-based deployments if the Pages project already has Git integration, so source preparation does not also publish an untagged build.

The workflow includes repository `LICENSE` and `NOTICE`, retains the exact candidate as an artifact before upload, and uses a pinned Wrangler action and CLI to upload to the Pages production branch. Each project serializes deployments without canceling an active release. Source revision and version appear in the run summary; command and upload outcomes remain in the owning job logs. Generated frontend `dist` is removed even after failure. A failed upload can be retried with the existing immutable tag. Actual Cloudflare deployment, domain routing and account configuration require a real platform run; local fixtures and builds do not establish those outcomes.

## Native publication handoff

Source preparation retains protected signing and publication gates. Preparing a FixKey version pushes its `fixkey@vMAJOR.MINOR.PATCH` tag and starts `release-fixkey.yml`. An unprotected Linux source job uses the trusted main helper to verify the tag format, version and main ancestry before allocating protected signing jobs or native runners. Native acceptance is recorded separately when performed; no native-acceptance revision variable or comparison blocks the workflow. To retry without another bump, dispatch `release-fixkey.yml` on main with the existing tag. All build and publication jobs check out the resolved tagged revision, including when main has advanced. Do not move the tag. Existing protected signing and publication environments, signed artifacts and immutable release checks remain required.

The DeliTools tag starts only its web publication. Dispatch `release-delitools-mobile.yml` on main separately with a new store build number and the desired platform. Signed build-only mode remains the default; Google Play internal testing and TestFlight uploads require explicit `publish`. Native builds, signing, device acceptance and store acceptance remain distinct from source preparation and web deployment.

Git Tracker client changes select `git-tracker-clients` on Ubuntu. The job checks the web app, Action tests/bundle freshness and shared generated bindings under `protos/`. Root `pnpm proto:generate` generates Go and TypeScript bindings together; both protocol and client freshness checks reject tracked and untracked changes in that tree. Protocol changes also select the client job because the web app consumes those schemas. Default manual validation remains affected-only.
