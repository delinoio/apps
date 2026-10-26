# Repository workflow contract

Generated protobuf and Connect bindings under `protos/` (`*.pb.go`, `*.connect.go` and `*_pb.ts`) are excluded from tokei counts through `.tokeignore` and from GitHub language statistics through `.gitattributes`. Schemas and hand-written files remain eligible for counting.

Root `pnpm dev` reads the resolved Compose configuration, runs `docker compose up -d --wait`, then starts `pnpm exec turbo run dev` directly to run the frontend and API tasks with a server-only database configuration handoff. Each workspace `dev` script registers its own named local URL through Portless. A Compose failure stops startup before any workspace task runs. The `dev` task is persistent and uncached. The current participants are DeliDev, DeliTools, DevBase, Delino Site, Delino Docs, Git Tracker and Perfine frontends, plus the Delcore, DevBase, Git Tracker, DeliTools and Perfine Rust APIs. Each retains its documented fixed loopback port and fails on conflicts. The API launchers set each service's fixed `LISTEN` before the server starts, overriding shared parent variables and service settings for development. Stop the development session with `Ctrl+C`. App-local `pnpm dev` and `pnpm dev:desktop` remain available for individual frontend and native development. Each API also supports service-local `pnpm dev`. All API launchers execute the Rust binaries built by the shared `//#server-build` task. Turbo builds the Rust workspace once before API startup. Restart `pnpm dev` to rebuild and apply server changes. Delcore loads its optional service-local `.env` for every invocation; DevBase, Git Tracker, DeliTools and Perfine load it through `--dev-env`. Turborepo passes each API's unprefixed runtime settings to its development task, plus `PERFINE_*`, `R2_*` and `ALERT_*` to Perfine, `ALERT_*` to Git Tracker, `DELITOOLS_*` and `AI_GATEWAY_*` to DeliTools, and `OTEL_*` and `NO_COLOR` to all APIs. Configure the existing provider requirements before starting. Root development supplies the local database URLs to the APIs after Infisical injection. Root development provisions the Compose databases; schema migrations and provider credentials remain prerequisites. See the [Delcore server contract](servers-delcore-api-contract.md#local-development) and [Git Tracker server contract](servers-git-tracker-api-contract.md#local-development) for configuration. Each app-local `dev` and `dev:*` command and each API `dev` command injects the linked Infisical project's `dev` variables before startup. Install and authenticate the CLI and initialize the local root project configuration as described in the [environment contract](repository-environment-contract.md#local-development-injection). Root development delegates injection to each workspace task. YAML-only catalog edits do not restart Delcore; restart its API task to apply them.

Shared protocol tooling lives in `protos/scripts/`: `check-breaking.mjs` runs FILE breaking checks against a comparison revision, and `buf-entry.mjs` resolves the installed Buf Node entry for that check and CI. Run `node protos/scripts/check-breaking.mjs "<comparison-revision>"` from the repository root. Localization fixture tests live beside their implementation in `scripts/localization/`; root `pnpm test:tooling` includes them.

Changes under `servers/scripts/` select all five API entries in both server validation matrices.

## Project-local agent skills

Project-local third-party skills are committed under `.agents/skills/`. Root `skills-lock.json` records their upstream sources, skill paths and content hashes. The installed skills are `turborepo` from `vercel/turborepo`, `vercel-react-best-practices` from `vercel-labs/agent-skills`, `vercel-composition-patterns` from `vercel-labs/agent-skills`, `postgres` from `planetscale/database-skills`, and the Lynx collection from `lynx-community/skills`, which is linked from the [official Lynx AI documentation](https://lynxjs.org/next/ai/). The Lynx collection includes `habitat-usage`, `lynx-a2ui`, `lynx-api-docs`, `lynx-check-css-support`, `lynx-debug-info-remapping`, `lynx-devtool`, `lynx-openui`, `lynx-trace-analysis`, `lynx-trace-record`, `lynx-typescript`, `lynx-ui`, `reactlynx-best-practices`, `rspeedy-bundle-size`, and `vanilla-lynx`.

Install these skills from the repository root:

```sh
npx skills add vercel/turborepo --skill turborepo --agent codex --yes
npx skills add vercel-labs/agent-skills --skill vercel-react-best-practices --agent codex --yes
npx skills add vercel-labs/agent-skills --skill vercel-composition-patterns --agent codex --yes
npx skills add planetscale/database-skills --skill postgres --agent codex --yes
npx skills add lynx-community/skills --skill '*' --agent codex --yes
```

The commands fetch the current upstream skills. Review content and lockfile changes before committing updates. Imported skills retain their upstream license metadata and available license files; see the [license contract](repository-license-contract.md). The shared formatter excludes `.agents/skills/` to preserve imported content.

## Worktree dependency sharing

Root `pnpm-workspace.yaml` sets `virtualStoreType: global` so worktrees share installed package contents through pnpm's global virtual store. Each worktree retains its own `node_modules` links and dependency versions selected by its lockfile.

Run `pnpm install` from each worktree root after checkout or dependency changes. pnpm reuses packages already present in the shared store. Run `pnpm store path` to locate the store; no machine-specific store path is committed. See the [pnpm worktree guide](https://pnpm.io/git-worktrees).

## Development task orchestration

The pinned Turborepo dependency manages development through `pnpm exec turbo run dev --concurrency=20 --continue=dependencies-successful`. No separate process supervisor is required. Frontend `dev` tasks remain persistent and use their existing HMR watchers. The five API `dev` tasks are persistent, and each depends on the root `//#server-build` task. The concurrency limit leaves room for all twelve persistent application tasks and the shared build.

The root launcher forwards `SIGINT` and `SIGTERM` to the Turbo process tree and waits for its descendants even if the immediate `pnpm` process exits first. On POSIX hosts, it owns a separate process group and sends `SIGKILL` to remaining group members after a ten-second grace period. On Windows, it terminates the tree with `taskkill`. The Compose databases and shared Portless proxy keep their separate lifetimes.

The root `server-build` script invokes `servers/scripts/build-rust.mjs`, the only Cargo build owner in the session. It skips compilation while the server workspace is empty. Install Cargo and `protoc` before root development. API launchers execute compiled binaries directly so they do not compete for the Cargo build lock. Selecting one API through a Turbo filter still includes the shared build dependency. See the [server Rust workspace contract](servers-rust-workspace-contract.md#shared-build-process) for inputs and scope.

The shared build declares server Rust source, Cargo manifests and lockfile, SQL, protocol source and the build helper as inputs. The server build has no root client-crate inputs. API task inputs include their package manifest and shared launch helpers. Turbo does not watch these inputs or automatically rebuild or restart APIs. Stop and rerun `pnpm dev` after server changes, including catalog YAML edits; frontend HMR continues through each frontend's own watcher.

A failed shared build blocks dependent API startup while independent frontend tasks continue to run. A failed application task does not stop other tasks whose dependencies succeeded. Turbo reports task failures and returns a nonzero exit code when the run finishes. Failed tasks do not restart automatically; correct the failure and rerun `pnpm dev`. Stop development with `Ctrl+C`; the databases remain running. Use `pnpm dev --ui=stream` for streaming logs, or `pnpm dev --filter=@delinoio/delcore-api` to select one API. Extra root `dev` arguments go to `turbo run`. Service-local development retains its existing entry points. Each frontend and API `dev` script registers its own Portless route after Infisical injection. The root launcher prepares the databases once, then Turbo owns task execution and dependency ordering.

## Local development databases

Root `compose.yaml` provides separate PostgreSQL 17 instances for Delcore, DevBase, Git Tracker, DeliTools and Perfine, matching the major version used by server integration CI. The frontends, native shells and Rust APIs run on the host through the existing development commands. Install Docker Engine and Docker Compose v2.20 or later, then start Docker. Root `pnpm dev` starts these databases automatically and waits for their healthchecks before launching Turbo. Existing containers and volumes are reused. `Ctrl+C` stops the development tasks and leaves the databases running for the next session. To start or inspect databases separately, run these commands from the repository root:

```sh
pnpm dev:db:up
docker compose ps
```

`pnpm dev:db:up` runs `docker compose up -d --wait`. These root database commands use the local Compose configuration directly and do not require Infisical. `--wait` waits for every database healthcheck to accept TCP connections. The probes use `127.0.0.1` inside each container so the PostgreSQL image's temporary socket-only initialization server cannot report readiness. To start only one database, append `delcore-postgres`, `devbase-postgres`, `git-tracker-postgres`, `delitools-postgres` or `perfine-postgres` to the `up` command. All databases publish only on `127.0.0.1`, use separate named data volumes and fail on port conflicts. The committed usernames and passwords are disposable local development values. Never use them for shared or production deployments.

- Delcore: `postgres://delcore:local-delcore@127.0.0.1:5432/delcore?sslmode=disable`
- Git Tracker: `postgres://git_tracker:local-git-tracker@127.0.0.1:55432/git_tracker?sslmode=disable`
- DeliTools: `postgres://delitools:local-delitools@127.0.0.1:55435/delitools?sslmode=disable`
- Perfine: `postgres://perfine:local-perfine@127.0.0.1:55436/perfine?sslmode=disable`

`scripts/dev.mjs` owns root Compose orchestration and URL resolution; `servers/scripts/dev-api.mjs` owns the API environment overrides. Root `pnpm dev` derives these URLs from the resolved Compose service credentials, database names and published host ports. After Infisical injection, every API launcher receives `DATABASE_URL` with the owning Compose URL. Delcore loads its service-local `.env` with lower priority than injected/process variables for every invocation; DevBase, Git Tracker and DeliTools do so when their launcher supplies `--dev-env`. The Node launcher leaves the file to the binary. The launcher preserves all other settings and does not edit the local file.

For direct service-local development, set each API's `DATABASE_URL` to its corresponding local URL yourself. Keep the remaining provider and authentication configuration under the owning [Delcore](servers-delcore-api-contract.md#local-development), [DevBase](servers-devbase-api-contract.md#local-development) and [Git Tracker](servers-git-tracker-api-contract.md#local-development) and [DeliTools](servers-delitools-api-contract.md#local-development) contracts. Compose does not provision Logto, Paddle, GitHub or email provider accounts. Configure Infisical as described in the [environment contract](repository-environment-contract.md#local-development-injection); Delcore also supports its ignored service-local `.env`.

Delcore development startup also applies its [split-file YAML catalog](servers-delcore-api-contract.md#development-catalog) after the explicit schema check and instance lock, before it listens or processes jobs. The default example is committed at `servers/delcore-api/catalog`; set `CATALOG_DIR` for another local directory. This applies equally through root `pnpm dev` and service-local `pnpm dev`, because both start the same API through its Rust launcher. Catalog application does not change Infisical injection, the root database handoff and its precedence, the fixed listener, or single-instance enforcement; a second instance fails at the lock before reading the catalog. YAML edits alone do not restart the API; restart its task. Catalog synchronization is not a migration: run `migrate` explicitly first. Fix invalid declarations or database constraints before restarting; no partial set is committed. Delcore is pre-release, so its development database may be reset instead of migrated; see the [pre-release transition](servers-delcore-api-contract.md#pre-release-transition) for the reset commands and the [recovery procedures](servers-delcore-api-contract.md#recovery-procedures) for catalog and metering failures.

Initialize each empty database explicitly before the first API start. From the repository root, with Delcore settings in its service-local `.env` and `DATABASE_URL` pointing to the local database, run:

```sh
cargo build --locked --manifest-path servers/Cargo.toml --workspace --bins
(cd "servers/delcore-api" && ../target/debug/delcore-api migrate --dev-env)
DATABASE_URL='postgres://devbase:local-devbase@127.0.0.1:55433/devbase?sslmode=disable' ./servers/target/debug/devbase-api migrate
DATABASE_URL='postgres://git_tracker:local-git-tracker@127.0.0.1:55432/git_tracker?sslmode=disable' ./servers/target/debug/git-tracker-api migrate
DATABASE_URL='postgres://delitools:local-delitools@127.0.0.1:55435/delitools?sslmode=disable' ./servers/target/debug/delitools-api migrate
pnpm dev
```

Delcore, DevBase and DeliTools track migration versions and checksums; rerun their migration commands after schema changes. Git Tracker currently embeds a single bootstrap SQL script with unconditional table and index creation. Run its migration command only once against an empty database. Do not rerun it on a retained volume after restarts or schema changes. Until upgrade migrations are supported, use a fresh disposable Git Tracker database for a changed schema; preserve any needed data before resetting its volume.

Database health does not establish schema readiness. Compose and root `pnpm dev` do not apply application migrations. Service-local `pnpm dev` can also use these databases; start Compose separately before using that command.

Inspect logs with `docker compose logs -f delcore-postgres devbase-postgres git-tracker-postgres delitools-postgres perfine-postgres`. Open a SQL session with one of these commands:

```sh
docker compose exec delcore-postgres psql -U delcore -d delcore
docker compose exec devbase-postgres psql -U devbase -d devbase
docker compose exec git-tracker-postgres psql -U git_tracker -d git_tracker
docker compose exec delitools-postgres psql -U delitools -d delitools
docker compose exec perfine-postgres psql -U perfine -d perfine
```

Stop and remove containers with `pnpm dev:db:down` (`docker compose down`); the data volumes remain. Run `pnpm dev:db:reset` to rebuild the server binaries, delete all local database volumes, restart the databases and apply all five APIs' migrations. `scripts/dev-db-reset.mjs` validates the resolved Compose database URLs and runs `cargo build --locked --workspace --bins` from `servers/` before deleting data. It uses `CARGO_TARGET_DIR` (relative to `servers/` unless absolute), or `servers/target`, for both build output and migration binaries. It then runs `docker compose down --volumes`, `docker compose up -d --wait`, and each API binary with `migrate --dev-env` from its service directory. Each migration receives its resolved local Compose `DATABASE_URL`, overriding parent and service-local database settings. Delcore still requires its full configuration in the process environment or ignored service-local `.env`; the other APIs require only the database URL for migration. Reset deletes all data in the selected Compose project and stops immediately on a failed or interrupted build, removal, startup or migration. It does not start APIs or apply the development catalog. Stop running APIs before reset. To delete volumes without restarting, use `docker compose down --volumes`. These commands respect `COMPOSE_PROJECT_NAME` and the existing host-port environment overrides.

For another checkout or occupied database ports, select an explicit project name and host ports on every Compose command. For example:

```sh
DELCORE_POSTGRES_PORT=15432 GIT_TRACKER_POSTGRES_PORT=15433 DEVBASE_POSTGRES_PORT=15434 DELITOOLS_POSTGRES_PORT=15435 PERFINE_POSTGRES_PORT=15436 docker compose -p apps-local-second up -d --wait
DELCORE_POSTGRES_PORT=15432 GIT_TRACKER_POSTGRES_PORT=15433 DEVBASE_POSTGRES_PORT=15434 DELITOOLS_POSTGRES_PORT=15435 PERFINE_POSTGRES_PORT=15436 docker compose -p apps-local-second down
```

When starting root development with that project, run `COMPOSE_PROJECT_NAME=apps-local-second DELCORE_POSTGRES_PORT=15432 GIT_TRACKER_POSTGRES_PORT=15433 DEVBASE_POSTGRES_PORT=15434 DELITOOLS_POSTGRES_PORT=15435 PERFINE_POSTGRES_PORT=15436 pnpm dev`. Root development derives the API database URLs from these ports automatically. Update the database URLs used by explicit migration commands and direct service-local development to the selected host ports. Each Compose project has its own data volumes. These overrides affect database ports only; application listeners retain their fixed ports, so concurrent worktrees cannot serve the same application on the host. PostgreSQL integration tests require separate disposable test databases through `TEST_DATABASE_URL`; do not point tests at databases containing development data.

See the Docker references for [Compose startup and healthchecks](https://docs.docker.com/compose/how-tos/startup-order/) and [PostgreSQL image storage and initialization](https://hub.docker.com/_/postgres).

## Named local development URLs

The root pins Portless as a development dependency. Run `pnpm dev` from the repository root to prepare the Compose databases, register local URLs for all seven frontends and the five Rust APIs, and start the Turbo development tasks. Each frontend and API package owns its route name and fixed upstream port in its `dev` script through `portless run --name <name> --app-port <port> <command>`. Infisical wraps that command so injection failures stop before route registration. The root launcher starts Turbo directly without Portless workspace discovery or re-entering the root script. Portless does not change the servers' listeners: keep Delcore configured on `127.0.0.1:8080`, DevBase on `127.0.0.1:46317`, Git Tracker on `127.0.0.1:46321` DeliTools on `127.0.0.1:46322` and Perfine on `127.0.0.1:46323`. The shared `//#server-build` task builds the server binaries before the APIs start. Root development starts the Compose databases; prepare each API's schema and provider configuration before starting. Each task retains its Infisical injection and the API environment allowlists. App-local and service-local `pnpm dev` register the same routes as root development. Native development hooks use the wrapped frontend scripts; their fixed direct loopback addresses remain available. Set `PORTLESS=0` when starting development to bypass the proxy and use those direct addresses. Turborepo passes `PORTLESS` and `PORTLESS_*` runtime settings to workspace tasks without adding them to build hashes.

| Application     | Main-checkout URL                   | Upstream          |
| --------------- | ----------------------------------- | ----------------- |
| DeliDev         | `https://delidev.localhost`         | `127.0.0.1:46311` |
| DeliTools       | `https://delitools.localhost`       | `127.0.0.1:46312` |
| DevBase         | `https://devbase.localhost`         | `127.0.0.1:46313` |
| Delino Site     | `https://delino-site.localhost`     | `127.0.0.1:46314` |
| Delino Docs     | `https://delino-docs.localhost`     | `127.0.0.1:46315` |
| Git Tracker     | `https://git-tracker.localhost`     | `127.0.0.1:46320` |
| Delcore API     | `https://delcore-api.localhost`     | `127.0.0.1:8080`  |
| DevBase API     | `https://devbase-api.localhost`     | `127.0.0.1:46317` |
| Git Tracker API | `https://git-tracker-api.localhost` | `127.0.0.1:46321` |
| DeliTools API   | `https://delitools-api.localhost`   | `127.0.0.1:46322` |
| Lane            | `https://lane.localhost`            | `127.0.0.1:46325` |
| Perfine API     | `https://perfine-api.localhost`     | `127.0.0.1:46323` |

Portless prefixes names with the branch in linked Git worktrees. Use the URLs printed at startup. Upstream ports remain fixed to preserve native shell development URLs and strict conflict handling; different worktrees cannot serve the same app concurrently on those ports.

Before the first session, run `pnpm exec portless proxy start` from an interactive terminal. Portless defaults to HTTPS on port 443, creates a local certificate authority, and can request administrator access to trust it and bind the port. This setup is local to the machine and is not an install hook. Non-interactive sessions must use an already prepared proxy. For HTTP without certificate setup or a privileged port, run `pnpm exec portless proxy start --no-tls --port 1355`; subsequent app commands reuse that proxy configuration and print URLs such as `http://devbase.localhost:1355`.

`Ctrl+C` stops the app session and removes its routes. The shared proxy remains available for other sessions. Inspect it with `pnpm exec portless list` or `pnpm exec portless doctor`; stop it with `pnpm exec portless proxy stop` when no session needs it. Portless keeps machine state outside the repository and can synchronize local host entries for browsers that need them. See the [upstream usage guide](https://github.com/vercel-labs/portless#readme) for trust and DNS troubleshooting. Rust APIs retain their separate startup commands, origins and authentication configuration.

### Git Tracker API configuration

The Git Tracker frontend also needs an API URL with the proxy's scheme and an exact matching allowed web origin. The direct-loopback values in the app and server `.env.example` files apply to direct development, not the Portless URLs. An HTTPS page cannot call the HTTP loopback API, and an HTTP Portless page still needs its own allowed origin.

Root `pnpm dev` starts the Git Tracker API through its Rust launcher and registers its route automatically. For the default HTTPS proxy in the main checkout, set `PUBLIC_GIT_TRACKER_API_URL=https://git-tracker-api.localhost` in the frontend's Infisical development configuration, or in the uncommitted `apps/git-tracker/.env.local` when the injected environment does not define it. Set `WEB_ORIGIN=https://git-tracker.localhost` in the API's Infisical development configuration, process environment or ignored service-local `.env`. Keep its other required settings under the [server contract](servers-git-tracker-api-contract.md). Restart the frontend and API after changing their settings. Portless does not supply credentials; the root development launcher provisions the Compose databases.

In a linked worktree, both web and API routes receive the branch prefix. Set `PUBLIC_GIT_TRACKER_API_URL` to the printed Git Tracker API origin and `WEB_ORIGIN` to the exact printed frontend origin. With the HTTP proxy on port 1355, the main-checkout API URL is `http://git-tracker-api.localhost:1355`; a linked worktree also includes its branch prefix. Include the scheme and any non-default proxy port, with no path or trailing slash. Register the frontend's `/callback` URL and sign-out origin in the development Logto application for the selected origin; authentication still requires its normal public application configuration.

Service-local `pnpm dev` registers the API route automatically. When running the compiled API binary directly instead, an optional static route remains available after starting the proxy:

```sh
pnpm exec portless alias git-tracker-api 46321
```

This explicit alias has no automatic worktree prefix and persists until removed with `pnpm exec portless alias --remove git-tracker-api`. Remove an existing static alias before starting root or service-local `pnpm dev`, whose API task manages the route and removes it on shutdown. Point the frontend at the static alias only while using that separate API workflow. Fixed upstream ports still permit only one local instance. Restore direct-loopback API and allowed-origin settings before returning to direct frontend development.

The local Delcore route supports server and command-line clients. The current Delcore handler does not provide CORS preflight responses or allow-origin headers, so browsers cannot make authenticated Connect calls to it from a different frontend origin. Do not set `PUBLIC_DELCORE_API_URL` to the local Portless origin for this workflow. Browser access to local Delcore requires separate CORS support; a registered Logto origin alone does not enable it. Keep `PUBLIC_DELCORE_RESOURCE` set to the resource identifier registered with Logto.

For server-to-server calls from Git Tracker to local Delcore, set `DELCORE_URL` to the printed Delcore API origin and retain its registered `DELCORE_RESOURCE`. Portless provides transport routing; it does not change resource identifiers or register development origins with identity providers. Follow the [Delcore server contract](servers-delcore-api-contract.md#local-development) for its remaining settings.

## Validation

The `shared-ui` job validates both `@delino/ui` and `@delino/lynx-ui` on Ubuntu.
Lynx UI source and owning contract changes select this job without selecting
unrelated application or server checks. The Lynx gallery builds web and native
bundles; these builds do not establish native host acceptance. Remove its
generated `packages/lynx-ui/dist` output after validation.

The root `package.json` pins pnpm 12.14.0; the shared CI Node setup uses the same version and a version-specific store cache. Dependency overrides and build-script decisions are configured in `pnpm-workspace.yaml`, because pnpm 12 does not read the `pnpm` field in `package.json`.

Install with `pnpm install --frozen-lockfile`. Run `pnpm test` in each app, root `cargo test`, and `go test ./...`. Generate frontend `dist` before Rust validation, then remove generated `dist` directories. Native builds, device launches, account-backed integration and store acceptance are separate checks. Preserve source revision, commands, results and unperformed checks in commits and CI artifacts.

CI compares PRs with their base, pushes with the previous revision, and manual runs with the main merge base (or the parent on main). Manual validation remains affected-only. Ordinary CI runs on Ubuntu; macOS is restricted to the separate main-only mobile release workflow, the dedicated main-only manual Perfine release procedure below and the Lane binary publication called by validated Delino tag/main recovery runs. Protocol validation includes Go vet, unit tests, consumer example builds and the DevBase TypeScript example type check; the dedicated formatting job checks Go formatting. Real PostgreSQL integration runs for the affected services under `servers/`. Ordinary CI does not run server OCI tests or release image builds. The Perfine adapter gate builds a disposable test-tool image only. Protocol CI requires lint, format and regeneration freshness; it does not run FILE breaking comparisons.

Mobile dispatch preserves signed build-only mode and opt-in Google Play internal/TestFlight uploads. Configure the protected environment described in the application contract before use. Release workflows do not establish store acceptance until run with real accounts.

The Tauri wrapper consumes the immutable release and hashes in `prebuilt-dependencies.lock.json`. These shared binaries remain published by `delinoio/oss`; keep that producer URL and source pin unchanged. Local source builds are explicit; CI source fallback is forbidden.

Use root `pnpm format` and `pnpm format:check` for changed supported files. Lefthook formats staged files while preserving unstaged changes. CI runs `pnpm format:check --all` across all tracked supported files without change detection. Track assets of at least 512 KiB with exact-path Git LFS entries and hydrate them before builds. Schema-only baseline checks skip unrelated LFS smudging.

Delino Site and Delino Docs run offline translation coverage and source-fingerprint checks in their owning `pnpm test` and `pnpm build` commands. Root `pnpm i18n:check` checks all workspaces that provide that script; root `pnpm i18n:translate` generates translations for up to two participating workspaces concurrently. Root account commands share the same local ChatGPT session as app-local commands. Shared localization fixtures participate in root `pnpm test:tooling`. Project-specific helpers and tests live under their owning app: DeliTools mobile workflow fixtures in `apps/delitools/scripts/mobile-workflow.test.mjs`. Root `pnpm test:tooling` and DeliTools `pnpm test` both run the mobile fixture suite. Translation generation and ChatGPT authentication are local maintainer operations under the [localization contract](apps-delino-site-localization-contract.md); CI and static deployment require no ChatGPT credentials.

The optional local breaking comparison uses the selected Git revision without rewriting historical `go_package` options. Go package paths participate in the FILE breaking checks alongside wire compatibility.

CI retains separate `devbase-frontend`, `delino-site-frontend`, `delino-docs-frontend`, `delidev-frontend`, `delitools-frontend`, `rust-clippy`, `rust-test` and `protocol` jobs, plus independent `rust-servers-clippy` and `rust-servers` matrices for Delcore, DevBase, Git Tracker, DeliTools and Perfine. The planner gates each project job before runner allocation. `check-formatting` (display name `Check Formatting`) owns all CI formatting checks: Rstack across all tracked supported files, both client and server workspace Rust formatting, and Go and Protobuf formatting. These checks and their toolchain setup always run without affected-path conditions or a dependency on the changes job. This job runs on Ubuntu and is required by `ci-result`. `tooling`, `shared-ui` and `workflows` are separate affected jobs for tooling fixtures, shared UI type/interaction/CSS tests and workflow syntax. The planner emits `tooling`, `shared_ui` and `workflows` outputs; each job uses its output before runner allocation and retains independent validation evidence. `ci-result` requires success only for selected shared checks and requires unselected checks to remain skipped. Changes in `packages/ui` select all frontend consumers without selecting server or protocol jobs. `ci-result` fails if any selected job fails, is canceled, or is skipped. Unselected project jobs must remain skipped.

The project planner excludes `AGENTS.md` and `CLAUDE.md` at every directory depth before selecting jobs. Shared paths are grouped by consumers: `packages/ui/` selects frontends; Cargo configuration and Rust toolchains select native products and Rust API servers; root Cargo manifests, prebuilt dependency locks and shared Tauri/prebuilt/archive helpers select native products; `go.mod` and `go.sum` select protocol validation; root Buf configuration selects protocol validation, API servers and Git Tracker clients; `scripts/localization/` selects Delino Site and Delino Docs. Unclassified shared paths, including CI tooling and root workspace configuration, retain global project selection. Multiple changed paths combine their selections. Formatting remains unconditional. Tooling paths include shared scripts, server launch helpers, DeliTools scripts, application package manifests, prebuilt dependency locks and GitHub workflows/actions. Shared UI paths select `shared-ui`; GitHub workflow/action paths and Go module changes select `workflows`. Unclassified shared paths select all three shared checks. Instruction-only changes select none of these affected jobs.

For `pnpm-lock.yaml`, the project planner reads both revisions from the exact event comparison before dependency installation. When only generated pnpm v9 importer blocks change, it selects their owners as package-manifest changes. Shared-workspace importer changes select the consumers of that workspace category. Root importer changes, edits outside importer blocks (including package resolutions, snapshots and settings), unknown ownership, unsupported layouts and unavailable comparisons retain global project selection. This supports pnpm's multi-document lockfile, including its package-manager document. Other changed paths still contribute their normal selection.

Root dependency locks select their owning runtimes rather than every project. `Cargo.lock` and `prebuilt-dependencies.lock.json` select the DeliDev and DeliTools project jobs; `go.sum` selects protocol validation, which includes Go consumer examples. These files alone do not select unrelated frontend or server jobs. Other changed paths still add their normal selection, and unrecognized root lockfiles retain shared-path selection.

Instruction-only comparisons emit empty Rust package lists without invoking cargo-mono. Instruction files also do not trigger the shared Rust tooling fallback in mixed comparisons. The changes job otherwise runs pinned cargo-mono twice against the planner base: `changed --direct-only` selects Clippy packages, and `changed` expands reverse workspace dependencies for Test. Each package list gates its own Rust job before runner allocation; empty lists skip the job. Workspace manifests and lockfiles retain cargo-mono global-impact handling. CI tooling, Cargo configuration, toolchain and shared prebuilt dependency changes select all Rust packages. `go.sum` alone does not select Rust validation. If cargo-mono resolves a different merge base from the event comparison, both checks conservatively select every workspace package. Frontend-only changes outside native crate directories do not select Rust checks.

Rust tests run one affected crate per matrix entry with `fail-fast: false`. Each entry uses the existing reverse-dependency selection, scopes Cargo to that crate and exposes its result and commands in the crate-specific Actions job logs without uploading a validation artifact. Clippy retains the direct-change package selection in one job. Its Linux apt installation runs only when a selected crate enables a native shell: CEF shells add GTK 4, GBM, X11 and portal dependencies. CEF build tools are installed only for CEF shell selections; default-feature test entries do not install shell prerequisites. Rust validation generates only the selected shells' frontend output, runs all-target tests and all-target/all-feature Clippy with warnings denied. Rust formatting runs in `check-formatting`. Cache restores are available to PRs; pnpm and Cargo caches are saved only after successful main validation. Each validation job except `rust-test` retains a JSON artifact with the source revision, commands, outcomes, and unperformed native/account checks; affected jobs also record their comparison. Protocol freshness rejects both tracked and untracked generated changes.

Server runtime validation runs in this checkout. `rust-servers-clippy` and `rust-servers` use Ubuntu runners and the independent server Cargo workspace for all five APIs. The planner emits only affected Rust services in the matrix and skips the job when the matrix is empty. Matrix entries run independently with `fail-fast: false` and retain separate validation artifacts with their service identity. Server and protocol changes select the affected service checks before runner allocation. Changes to `docs/protos-delcore-v1-contract.md` select Delcore server validation; changes to `docs/protos-devbase-v1-contract.md` select DevBase server validation; changes to `docs/protos-git-tracker-v1-contract.md` select Git Tracker server validation; changes to `docs/project-delitools.md`, `docs/servers-delitools-api-contract.md`, `docs/protos-delitools-v1-contract.md` and `examples/delitools*` select DeliTools server validation. The `delitools-frontend` job is selected by `docs/project-delitools.md` and `docs/apps-delitools-*` but not by the server and protocol contracts, and the `protocol` job also builds `examples/delitools-client`. Protocol source changes select every server entry, so API key changes in `protos/delcore/v1` also validate DeliTools. `rust-servers-clippy` runs service-specific Cargo Clippy with all targets and warnings denied. The independent `rust-servers` job runs unit tests and PostgreSQL tests. Its PostgreSQL step streams test output and has a ten-minute timeout so a stalled fixture fails before the runner job limit while preserving validation evidence and database cleanup. Both jobs use the same affected-service matrix, retain separate validation artifacts and are required by `ci-result` when selected. Neither job installs Go or runs Go compatibility checks. OCI test commands remain available for explicit local packaging validation. Go formatting remains in `check-formatting`. `ci-result` rejects failed, canceled or skipped selected server jobs. Record source revision, comparison, commands, results and unperformed live-account checks in the same review and CI evidence. See the [repository boundary contract](repository-boundary-contract.md).

### Perfine repository validation

Perfine has explicit affected ownership for CLI/distribution, fixture profiles and hosted API. Changes in `crates/perfine-core` select local Rust validation and profiling gates without selecting the Perfine server matrix entry. Root Cargo manifest changes do not select the hosted consumer. Cargo-mono expands root engine changes to the `perfine` CLI for Rust tests; Perfine contract, protocol and distribution changes select the available `perfine-core` and `perfine` packages explicitly. Shared server inputs still select every API. Perfine protocol changes select protocol and client consumers.

Action and CLI distribution tests and builds run locally and in the manual release workflow. Ordinary CI has no Perfine distribution job. Perfine server Clippy, unit tests and ignored PostgreSQL tests use the existing independent server matrices with package `perfine-api` and a disposable `perfine_api_test` database on port `55436`.

`perfine-profiles` runs the real driver at `crates/perfine-core/tests/fixtures/run.py` on Linux x86_64 for Rust, C, C++, Zig, Go and JavaScript. Native languages exercise both Callgrind and Gungraun routes. Each matrix entry measures the bulk and protected cases with baseline, same-source baseline repeat and candidate variants. CI provisions compilers, Python and Valgrind, records their versions, uses the repository Rust/Go/Node selectors, verifies the Zig 0.15.2 archive checksum, and installs Gungraun runner 0.19.4. The fixture harness pins its dependencies separately. No exploratory host override or JIT disabling is permitted in this gate. The Zig archive pin comes from the [official download index](https://ziglang.org/download/index.json).

The gate preserves driver results, command logs and profile artifacts, excluding compiled binaries and build caches. Its execution record includes the planner base/head, checkout revision, platform and each command outcome. The driver's source hashes distinguish materialized variants from Git candidate commits. Driver execution does not run the engine lifecycle or assign a performance verdict; its verdict remains `not_evaluated`. Engine policy and lifecycle checks remain in the owning Rust tests. Do not treat these fixture profiles as complete local optimization or live-provider acceptance.

`perfine-adapters` runs `node scripts/ci/perfine-acceptance.mjs <language>` in a six-language Linux x86_64 matrix. The runner provisions digest-pinned Rust, Go and Node image inputs plus checksum-verified Zig, then passes immutable local image IDs to the real CLI Docker adapters. It invokes the ignored `six_language_<language>` test in `cmds/perfine/tests/local.rs`. A deterministic structured-response agent runs on an internal Docker network without provider credentials. Each language exercises same-SHA baselines, profile-linked hypotheses and candidate commits, independent correctness, bulk/protected measurements, policy verdicts, reverse regression, no candidate and inconclusive outcomes from an explicitly injected review conflict with no repeat evidence. Go and Node use a declared repeated timing policy; Node retains normal JIT. Timing assertions accept an inconclusive verdict only when retained variability exceeds that frozen policy, or when the test explicitly injects a review conflict. A successful fixture test does not imply a performance pass. The runner preserves image IDs, source revision, commands, settings, CLI output and engine artifacts even on failure. The C entry also runs `cargo test --locked -p perfine-core --test adapter_execution -- --ignored --nocapture`. These tests complement the driver and core adapter tests; the mocked agent does not establish live AI acceptance. An explicit `PERFINE_ALLOW_EMULATED=1` local override records emulation and must never be used by the native CI gate. Both Perfine fixture jobs and selected server matrix results are required by `ci-result`; failed, canceled or skipped selected jobs fail the result gate. Manual runs remain affected-only, and ordinary runners remain Linux. These gates do not publish npm packages, Actions, server images or deployments. Live GitHub/App/OIDC/AI and installation acceptance must be recorded separately.

### Perfine release validation

The dedicated manual `release-perfine.yml` procedure is separate from PR, main-push and affected manual CI. Ordinary CI remains affected-only with its existing Linux gates. The macOS exception applies only to Perfine release preparation and validation; it does not introduce a full manual CI mode or change the owning mobile release gates.

Prepare the immutable main-ancestor source tag on all four native host targets:

| Runner           | CLI target       |
| ---------------- | ---------------- |
| `ubuntu-24.04`   | Linux x86_64 GNU |
| `macos-15-intel` | macOS x86_64     |
| `macos-15`       | macOS ARM64      |
| `windows-2025`   | Windows x86_64   |

Each target must install the actual npm entry archive and its exact matching optional platform dependency offline in a disposable directory, then extract the standalone archive. Check archive and binary checksums, notices, provenance, executable startup, `--version`, and structured missing-configuration `doctor` output through both entry points. All four required checks must succeed before publication; failed, canceled or skipped required checks block publication. Compilation, fixture binaries and cross-compilation alone do not satisfy these native checks.

Retain separate records for package/executable smoke checks, minimum-OS execution, full Docker workflows and live-account checks. Include the source revision, actual OS/build and CPU, commands, results and unperformed limits. The `windows-2025` Windows Server runner does not prove Windows 11 25H2/build 26200 compatibility. Mark that minimum unverified until execution on the required Windows 11 environment is recorded. Likewise, a newer runner alone does not verify any minimum. Support verification remains incomplete while a required minimum environment is unavailable.

Manual Docker checks cover each host's baseline/candidate measurement, comparison, resume/cancel, unavailable daemon, missing images/tools, wrong architecture, inaccessible mounts and authentication prerequisites. Record ARM Mac x86_64 emulation distinctly from native Linux x86_64 evidence, and mocked agents distinctly from live providers. Unperformed full Docker or live-provider checks remain explicit but do not block publication. Do not waive the required native package/executable checks or publishing-authority checks on that basis.

Preserve default preparation-only `publish: false`, explicit publication opt-in, main-only dispatch, the protected `perfine-production` environment, immutable source/distribution tags, package versions and release assets, and manual partial-publication reconciliation. Verify publishing authority for `perfine` and all four approved platform names before publication; absent public names or failed registry authentication do not establish authority. Never overwrite artifacts to repair a release. Follow the [distribution contract](packages-perfine-distribution-contract.md#release-preparation-and-exact-commit-verification) for exact Action commit verification and release ordering.

### Perfine local development

`perfine-postgres` uses a separate disposable PostgreSQL volume and loopback port `55436`; set `PERFINE_POSTGRES_PORT` to override it. Root development resolves its URL from Compose into the `perfine` entry of `DEV_COMPOSE_DATABASE_URLS`. Only the API development task receives this handoff. Its owning server launcher applies `DATABASE_URL` after Infisical injection, fixes `LISTEN` to `127.0.0.1:46323`, and executes `perfine-api serve --dev-env` from the shared server build. It must leave service-local files unchanged. Explicit schema migrations remain the server's responsibility; root development only waits for database health. See the [Perfine server contract](servers-perfine-api-contract.md) for operator prerequisites.

## Turborepo remote caching

Use Vercel Remote Cache for cached Turbo builds. Configure the repository Actions variable `TURBO_TEAM` and secret `TURBO_TOKEN` under the [environment contract](repository-environment-contract.md#turborepo-remote-cache). CI runs the frontend preparation builds in `rust-clippy` and `rust-test` through `pnpm exec turbo run build:frontend --filter=<workspace>`. The existing affected-job and package gates remain in place. Frontend test commands and native Rust validation still execute normally; they are not cached by this integration.

Main pushes can read and write remote artifacts. Affected manual runs on main use remote reads only. PRs, including same-repository PRs, and non-main manual runs receive no remote cache token and use `TURBO_CACHE=local:rw`. The token itself grants write access; Turbo read-only options do not reduce its authorization and cannot protect it from PR-controlled commands. If either credential setting is absent, trusted main jobs also use local caching. This policy does not change pnpm or Cargo cache handling. Turbo uploads each successful build task independently of later job results. Credentials and cached logs must never contain account data or signing material.

Build tasks restore `dist/**` and include ignored `.env*` files in their inputs. Shared UI source, localization tooling, root license notices, the Node selector and pnpm workspace configuration participate in the global hash alongside the existing Go and protocol inputs. `NODE_ENV`, `PUBLIC_*`, `RUNNER_OS` and `RUNNER_ARCH` values also participate in the hash. CI artifacts are therefore isolated by runner OS and architecture; local runs without those runner variables use separate entries. Keep additional build-affecting environment variables and external files in the owning Turbo inputs before using them in cached builds. Development and test tasks remain uncached.

For local builds, run `pnpm exec turbo login`, then `pnpm exec turbo link` from the repository root and select the same Vercel team. Local authentication and `.turbo/config.json` remain uncommitted. Run `pnpm exec turbo run build:frontend --filter=<workspace>` to cache a frontend preparation build, or root `pnpm build` for the existing workspace build tasks. Remove the generated `dist` directories after validation. To bypass remote access, set `TURBO_CACHE=local:rw`; use `--force` to execute a task despite a cache hit. Login and link require interactive Vercel account access.

See the [upstream GitHub Actions guide](https://turborepo.dev/docs/guides/ci-vendors/github-actions) and [cache controls](https://turborepo.dev/docs/reference/run#--cache-options).

## Project release preparation

`.github/workflows/release-project.yml` follows the shared preparation and project-specific publication pattern. Dispatch it from `main`, select `devbase`, `delino` or `delitools`, and choose `patch`, `minor` or `major`. Only the selected release's application versions change. The `delino` release calculates the next version from `apps/delino-site/package.json` and writes it to both that manifest and `apps/delino-docs/package.json`, regardless of their previous version agreement. Other web-only releases update their own npm manifest. DeliTools also updates its Tauri configuration, native package manifest and corresponding workspace `Cargo.lock` entry. Native source versions must agree before preparation.

`scripts/release/project.mjs` owns version calculation, source validation and the Git transaction. Preparation uses the release bot's identity, commits the selected files and atomically pushes `main` and its immutable tag. A concurrent main change or an existing tag rejects publication without moving the remote branch or tag. Preparation is serialized across projects. The commit records the workflow run ID and bump; rerunning the same run validates and reuses its recorded source even if main has advanced. Changed inputs, source contents or tags are rejected. Start a new run for a new version. Do not replace a tag or bypass branch protection to recover a failed run.

Tags are `devbase@vMAJOR.MINOR.PATCH`, `delino@vMAJOR.MINOR.PATCH` and `delitools@vMAJOR.MINOR.PATCH`. A GitHub App installation for this repository supplies `DELINO_RELEASE_BOT_CLIENT_ID` as an Actions variable and `DELINO_RELEASE_BOT_PRIVATE_KEY` as a secret. It requires contents write access and repository rules that permit its release commits and tag creation. The app token allows tag pushes to start the owning workflows; the default Actions token does not trigger that follow-up automation. Checkout temporarily retains the app credential for the atomic push and removes it during post-job cleanup. Registry package publication occurs only in the `delino` release workflow described under [Server image publication](#server-image-publication).

DeliDev has no release entry because its owning desktop contract removes distribution automation. Server runtimes live under `servers/`. Only the `delino` tag publishes their images; see [Server image publication](#server-image-publication). Release preparation itself does not publish images.

## Web publication

The individual `.github/workflows/release-devbase.yml`, `release-delino.yml` and `release-delitools.yml` workflows publish the corresponding web applications. Each starts from its release tag. `release-delino.yml` publishes `delino-site`, `delino-docs`, `devbase`, `git-tracker` and `lane` in parallel from the same `delino@vMAJOR.MINOR.PATCH` tag and source revision. Source validation checks the `delino-site` and `delino-docs` package versions; `devbase`, `git-tracker` and `lane` keep their own independent versions and are built from the tagged revision. The `devbase` application is also published by its own `release-devbase.yml`, which remains available for web-only releases. The former `delino-site@v*` trigger is retired. To retry a deployment without another bump, dispatch the individual workflow from `main` with the existing tag. A source job checks the tag format, version and main ancestry using the trusted main helper before allocating the protected deployment job. Each deployment job checks out the resolved immutable source revision, hydrates LFS objects, installs the frozen workspace and runs the app's `pnpm test` to check types and build the static output. The `delino` release workflow runs only the app's `pnpm build`.

The Delino deployment matrix keeps `fail-fast: false` so one app failure does not cancel the others. The Cloudflare uploads are not atomic; inspect every job and retry the existing tag if any fails.

The Delino Wrangler action sets `pnpm_config_ignore_workspace_root_check=true` only for its deployment step because the pinned action installs its CLI with `pnpm add` at the workspace root without `--workspace-root`. pnpm 12 requires the `pnpm_config_` prefix for this setting. This exception allows that temporary CLI installation without changing the workspace-wide pnpm configuration. Remove it when the action supports explicit workspace-root installation. The workspace build-script allowlist permits `workerd`, which Wrangler installs during deployment.

DevBase and DeliTools production deployments use the corresponding `<project>-production` GitHub environment with secret `CLOUDFLARE_API_TOKEN` and variables `CLOUDFLARE_ACCOUNT_ID` and `CLOUDFLARE_PAGES_PROJECT`. The `delino` release shares the `delino-site-production` environment among Delino Site, Delino Docs, Git Tracker and Lane with secret `CLOUDFLARE_API_TOKEN` and variable `CLOUDFLARE_ACCOUNT_ID`. Its `CLOUDFLARE_PAGES_PROJECT_SITE`, `CLOUDFLARE_PAGES_PROJECT_DOCS`, `CLOUDFLARE_PAGES_PROJECT_GIT_TRACKER` and `CLOUDFLARE_PAGES_PROJECT_LANE` variables select the marketing, documentation, Git Tracker and Lane Pages projects. The DevBase application in the same release deploys through its owning `devbase-production` environment and the `release-devbase` concurrency group, exactly like `release-devbase.yml`, so one reviewer gate and one serialized upload queue protect its Pages project. `devbase-production` must also permit `delino@v*` tags and main-dispatched recovery. Each matrix entry names its environment and concurrency group. The Git Tracker build also reads the public browser identifiers `PUBLIC_LOGTO_ENDPOINT` and `PUBLIC_LOGTO_APP_ID` from this environment. Without them the Git Tracker sign-in is not configured; the workflow does not validate them. These values are not secrets. Lane builds with its default public API origin `https://lane.api.delino.io`; no server credentials enter its web build. Its Pages project uses production branch `main` and domain `lane.delino.io`. The deployment matrix maps each application to its own variable for both configuration validation and upload. The token needs Cloudflare Pages edit permission for the owning account. Configure reviewers and deployment branch/tag restrictions to permit the owning project tags and main-dispatched recovery. `delino-site-production` must permit `delino@v*` tags and main-dispatched recovery. Create each Pages project with production branch `main` and bind its documented domain before publication. Disable automatic Git-based deployments if the Pages project already has Git integration, so source preparation does not also publish an untagged build.

The workflow includes repository `LICENSE` and `NOTICE`, retains the exact candidate as an artifact before upload, and uses a pinned Wrangler action and CLI to upload to the Pages production branch. Each project serializes deployments without canceling an active release. Source revision and version appear in the run summary; command and upload outcomes remain in the owning job logs. Generated frontend `dist` is removed even after failure. A failed upload can be retried with the existing immutable tag. Actual Cloudflare deployment, domain routing and account configuration require a real platform run; local fixtures and builds do not establish those outcomes.

## Server image publication

A `delino@vMAJOR.MINOR.PATCH` release also publishes the five server images. The `images` job runs after the shared source job, in parallel with the web publication. It builds `servers/delcore-api`, `servers/devbase-api`, `servers/git-tracker-api`, `servers/delitools-api` and `servers/lane-api` from their Dockerfiles at the tagged revision and publishes `ghcr.io/<owner>/<server>:<version>` with the repository's `GITHUB_TOKEN`. Only this job holds `packages: write`. The image labels carry the source repository, version, revision and server license.

A release tag never moves, so a retry reuses an existing image tag instead of rebuilding it. Retry a failed run by dispatching `release-delino.yml` from `main` with the existing tag. The release does not deploy the images or run migrations; operators deploy them and apply migrations as the server contracts require. The Git Tracker migration is a one-time initial schema and is not repeatable. DeliTools provides fix-forward recovery only: it has no previous-image rollback or reverse migrations, and its operator procedures live in the [server contract](servers-delitools-api-contract.md#operator-procedures). Make the published GHCR packages readable by the deployment target, either public or with registry credentials held by that target.

GHCR package creation and visibility have not been exercised against a real account. Record the first real release run as the platform acceptance evidence.

## Lane binary publication

`release-delino.yml` calls the reusable `release-lane.yml` after immutable tag/main
ancestry validation. It builds CLI binaries for Linux amd64/arm64, macOS
amd64/arm64 and Windows amd64/arm64 at the resolved source revision. Linux and
macOS targets execute on matching architecture runners; Windows ARM64 is
cross-built on the Windows x64 runner and is not executed there. Each host runs
the package tests and the real packed CLI version/help/MCP smoke checks.

The two macOS runners and Windows x64 runner also build the desktop archive with
`LANE_URL_SCHEME=lane`, the production bundle, native host, Chrome relay and
matching verified CLI synchronization helper. macOS archives retain framework
symlinks and ad-hoc signatures; Windows is an unsigned portable application
archive. The owning desktop build launcher prepares its pinned Windows App SDK
and C++/WinRT packages and generates both Windows OS and App SDK projections.
The release workflow leaves the SDK override variables unset so that this
automatic preparation remains active.
Windows ARM64 GUI, formal installers, signing/notarization and mobile store
publication are separate. The macOS runner exception applies only to this owning
release workflow, never PR, main-push or affected manual CI.

Host candidates travel as tar archives so artifact storage does not discard
executable modes or symlinks. Assembly rejects missing/duplicate targets, mixed
source revisions, version mismatches and corrupted executable or desktop bytes.
It requires all six CLI targets and all three desktop archives before preparing
the lightweight npm launcher and six platform npm archives, direct CLI archives, `lane-release.json` and
`SHA256SUMS`. CLI/npm versions remain those committed in Lane, independent of
the Delino release version; desktop archive names carry the Delino tag version.
Desktop packages also embed the exact release tag and source revision for
in-app update comparison and candidate verification. Desktop packaging rejects
a missing installer or mismatched embedded release identity. Follow the
[desktop update contract](apps-lane-foundation.md#desktop-app-updates).

The publication job uses `delino-site-production` and is the only Lane binary job
with `contents: write`. It creates a draft GitHub Release on the existing Delino
tag, uploads all verified candidates and publishes the draft after successful
uploads. No additional stored credential is required. The npm archive is a
GitHub Release download; this workflow does not publish to the npm registry.
Direct CLI assembly uses sorted file entries, fixed ownership and timestamps,
and a deterministic gzip header. Reassembling the same retained candidates
produces byte-identical assets and checksums. Rerun failed jobs to reuse retained
candidates after an upload failure. Existing
assets are downloaded and checked byte-for-byte before reuse; never overwrite
them. A rebuilt candidate that differs from a published asset fails and requires
a new release tag. CLI smoke checks and desktop builds do not establish native
installation, GUI behavior, provider delivery, account acceptance or formal
signing/store acceptance. Record the source revision, commands and actual
platform outcomes in the owning job logs and summaries. Generated `dist` and
desktop build directories are removed even on failure.

## Native publication handoff

The DeliTools tag starts only its web publication. Dispatch `release-delitools-mobile.yml` on main separately with a new store build number and the desired platform. Signed build-only mode remains the default; Google Play internal testing and TestFlight uploads require explicit `publish`. Native builds, signing, device acceptance and store acceptance remain distinct from source preparation and web deployment.

Git Tracker client changes select `git-tracker-clients` on Ubuntu. The job checks the web app, Action tests/bundle freshness and shared generated bindings under `protos/`. Root `pnpm proto:generate` generates Go and TypeScript bindings together; both protocol and client freshness checks reject tracked and untracked changes in that tree. Protocol changes also select the client job because the web app consumes those schemas. Default manual validation remains affected-only.

Delcore, DevBase, Git Tracker and DeliTools use the [server Rust workspace](servers-rust-workspace-contract.md). Their development launchers execute compiled binaries. Delcore retains optional working-directory `.env` loading for every invocation; DevBase, Git Tracker, DeliTools and Perfine load it only with `--dev-env`. Shared crate changes select all five Rust API matrix entries. Their Go service implementations, Go test oracles and Air configurations are removed; the root Go module continues to own generated protocol bindings and Go consumers.

### Lane validation

Lane source, Chrome extension source and owning contract changes select the Ubuntu `lane-frontend` job.
It checks TypeScript, builds native and browser bundles, runs the Chrome extension tests/build, and runs the desktop RealQA bridge fixtures with Node. It removes both app and extension generated `dist` output even on failure. These checks do not
establish native SDK compilation or device/store acceptance. Native host build
commands and prerequisites belong to the [Lane contract](apps-lane-foundation.md).

## Lane development and validation

The affected Lane API test job also runs `servers/lane-api/ops/check.sh` with disposable Prometheus and Alertmanager tool containers plus the Node routing/probe tests. This validates monitoring configuration and simulated alert behavior without starting the service or contacting providers.

Lane participates in root development through its existing `lane` frontend route and the `lane-api` Rust service route. The API uses fixed loopback port `46326`; the frontend keeps `46325`. Root Compose includes `lane-postgres`, with loopback port `${LANE_POSTGRES_PORT:-55437}`, database/user `lane` and disposable password `local-lane`. Its named `lane-postgres-data` volume is independent of other services. These values are local development defaults only.

Run `pnpm dev:db:up` to prepare the local databases. Run the Lane binary's `migrate` command explicitly before serving; application startup does not migrate. Root `pnpm dev` waits for Compose, then runs Turbo directly. Only the Lane API task receives its resolved database URL, after service-local Infisical injection. Root `pnpm dev:db:reset` includes the Lane database in its existing build, reset and explicit migration procedure. Do not run that destructive reset merely to validate a change; tests use disposable data.

The [Lane project](project-lane.md) and [API contract](servers-lane-api-contract.md) own product scope and service configuration. Native build/install/behavior acceptance remains separate from frontend compilation and fixture checks.

Lane CI uses the existing `lane-frontend` job, an affected `lane-cli` job, and a `lane-api` entry in both Rust server matrices. CLI/package changes select Rust tests and package tests/builds for the Linux amd64 host on Ubuntu; full six-target packaging requires the corresponding Rust target, linker and native SDK toolchains. Root Cargo changes also select CLI validation; local-store schema changes select both CLI and frontend consumers. Lane protocol changes select its app, CLI and server consumers plus protocol validation. Shared Lynx UI changes select Lane frontend validation. The result gate rejects failed, canceled or skipped selected Lane jobs. The API matrix uses disposable database `lane_api_test` on port `55437`. Its existing integration step invokes `servers/lane-api/tests/integration_fixture.py` to supply a pinned disposable loopback S3 mock while running all Lane ignored tests. See the [server fixture procedure](servers-lane-api-contract.md#disposable-s3-integration-fixture) for prerequisites and cleanup. These checks do not add macOS runners, a full manual mode or production publication.

## Pending changesets

The [changeset contract](repository-changeset-contract.md) owns pending change
records and the Go CLI under `tools/changeset/`. Use npm package names for all
component identities. Public changelog previews select explicitly authored public
notes only. The unconditional formatting job validates records, runs CLI tests
and vet, and checks Go formatting in this directory. Release preparation and
record deletion remain separate from this workflow.
