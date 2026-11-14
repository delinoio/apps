# Repository Deployment Contract

## Scope

This contract owns production deployment for every deployable component in the repository: the GitHub Actions deployment workflows, the GitHub environments and their credentials, the component dependency order and the hosting targets. Project indexes and domain contracts own each component's build, runtime configuration and origin. The [workflow contract](repository-workflow-contract.md) owns pull request validation.

## GitHub environments

The repository has exactly two GitHub environments. Do not add per-project, per-component or per-stage environments. A new static site uses `static-sites`, and a new server uses `servers`.

| Environment ID | Target                  | Components                                          | Secrets                | Variables               |
| -------------- | ----------------------- | --------------------------------------------------- | ---------------------- | ----------------------- |
| `static-sites` | Cloudflare Pages        | `delino-site`, `delino-docs`, `delino-accounts-web` | `CLOUDFLARE_API_TOKEN` | `CLOUDFLARE_ACCOUNT_ID` |
| `servers`      | Railway (Docker builds) | `accounts-api`                                      | `RAILWAY_TOKEN`        | none                    |

- `CLOUDFLARE_API_TOKEN` is a Cloudflare API token with the `Cloudflare Pages: Edit` account permission only. `CLOUDFLARE_ACCOUNT_ID` is the Cloudflare account ID. It is not a secret.
- `RAILWAY_TOKEN` is a Railway project token for the `production` environment of the shared Railway project. Project tokens can deploy only into that project environment. Every server is a service in that one Railway project, so one token serves all servers. Do not use an account or workspace token.
- Both environments restrict deployment branches to `main`. Credentials exist only as environment secrets and variables. Do not store them as repository secrets, in the repository or in public documentation.
- Only the deployment workflows reference these environments. Pull request validation never receives deployment credentials.

## Components and dependencies

| Component ID          | Path                    | Environment    | Hosting resource                   | Origin                           | Depends on     |
| --------------------- | ----------------------- | -------------- | ---------------------------------- | -------------------------------- | -------------- |
| `accounts-api`        | `servers/accounts-api/` | `servers`      | Railway service `accounts-api`     | `https://accounts.api.delino.io` | none           |
| `delino-accounts-web` | `apps/delino-accounts/` | `static-sites` | Cloudflare Pages `delino-accounts` | `https://accounts.delino.io`     | `accounts-api` |
| `delino-site`         | `apps/delino-site/`     | `static-sites` | Cloudflare Pages `delino-site`     | `https://delino.io`              | none           |
| `delino-docs`         | `apps/delino-docs/`     | `static-sites` | Cloudflare Pages `delino-docs`     | `https://docs.delino.io`         | none           |

- A component depends on another component when it calls that component at runtime. `delino-accounts-web` calls `accounts-api`, so the API deploys first and a web build that uses a new API capability never goes live before that capability. Links between sites by absolute URL are not deployment dependencies.
- A component deploys only after each dependency deployed successfully in the same run or was not selected for the run. When a dependency fails or is cancelled, its dependents do not deploy.
- Components without a dependency path between them deploy in parallel.
- Add a new deployable component to this table, to the planner in `scripts/ci/deploy-plan.mjs` and to `.github/workflows/deploy.yml` in the same change. Add a dependency edge in all three places when a component starts to call another component.

## Deployment workflow

`.github/workflows/deploy.yml` runs on pushes to `main` and on manual dispatch from `main`. Runs from other refs fail in the plan job.

1. The `plan` job selects components with `scripts/ci/deploy-plan.mjs`. On a push, it compares the current revision with the head revision of the last fully successful run of the workflow on `main`. Changes from failed runs, cancelled runs and queued runs that GitHub replaced stay in the comparison until a run succeeds. The plan selects every component in these cases: manual dispatch, no successful run, an unknown revision, a revision that is not an ancestor of the current one, and changes to the deployment workflows or planner.
2. The planner selects a component when a changed, deleted or renamed path is in the component directory or in its shared inputs. Static sites share the root Node workspace files `.nvmrc`, `package.json`, `pnpm-lock.yaml`, `pnpm-workspace.yaml` and `turbo.json`. `accounts-api` reads the root Go module files `go.mod` and `go.sum`.
3. Each selected component validates and builds, then deploys in dependency order. The plan record and each deployment record the revision, commands and result in the job summary.

The workflow uses the `deploy-production` concurrency group without cancelling in-progress runs, so a deployment is never interrupted.

### Static sites

`.github/workflows/deploy-cloudflare-pages.yml` is a reusable workflow for one static package in the `static-sites` environment. It runs `pnpm install --frozen-lockfile --ignore-scripts` and `pnpm exec turbo run typecheck test build --filter=<package>`. Then the pinned Wrangler CLI uploads the output directory with `wrangler pages deploy <dir> --project-name=<project> --branch=main --commit-hash=<sha>`.

Each Cloudflare Pages project is a Direct Upload project with production branch `main` and its custom domain from the components table. Pages projects must not use the Cloudflare Git integration, because Git-triggered builds bypass the dependency order.

### Servers

Servers deploy in the `servers` environment. The `accounts-api` job runs `go vet` and `go test` for the server packages. Then the pinned Railway CLI runs `railway up --service accounts-api --ci` from the repository root. The upload includes the repository root without Git-ignored files. `--ci` exits successfully only after the Railway deployment reaches `SUCCESS`, which includes the health check. A build failure, health check failure or crash fails the job, and dependents do not deploy.

Railway builds the server's Dockerfile with the repository root as the build context. Railway keeps the previous deployment active until the new deployment passes its health check. Configure each Railway service in the dashboard:

- No connected GitHub source. Deployments come only from the workflow, so automatic deploys do not bypass the dependency order.
- Service variable `RAILWAY_DOCKERFILE_PATH` set to the server Dockerfile path, for example `servers/accounts-api/Dockerfile`.
- Service variable `PORT` equal to the container listen port, so the health check and public routing reach the server.
- Health check path set to the server's health endpoint.
- Custom domain from the components table. Railway terminates TLS.

The server's domain contract owns the values of these settings and its runtime variables.

Railway Config as Code files (`railway.json`, `railway.toml`) are not used, because Railway stops reading them on 2026-12-01. Railway Infrastructure as Code (`.railway/railway.ts`) is not adopted yet. Adopting it requires an account-scoped token for `railway config apply`, so it needs a separate credential decision.

## Tool versions

The workflows pin the Wrangler and Railway CLI versions in workflow-level `env` values and pin GitHub Actions to commit SHAs. Update a CLI pin to a release that is at least two weeks old, and record the change in the pull request.

## Validation records

Each deployment job records the revision, commands and result in its job summary. Fixture tests for the planner run through `pnpm test:tooling`. A successful deployment job proves only that the hosting provider accepted the deployment and, for servers, that the health check passed. Record product acceptance in pull requests, Linear issues or CI artifacts.

## References

- [repository-workflow-contract.md](repository-workflow-contract.md)
- [repository-environment-contract.md](repository-environment-contract.md)
- [project-delino-site.md](project-delino-site.md)
- [project-delino-docs.md](project-delino-docs.md)
- [project-delino-accounts.md](project-delino-accounts.md)
- [servers-accounts-api-foundation.md](servers-accounts-api-foundation.md)
- [Cloudflare Pages Direct Upload](https://developers.cloudflare.com/pages/get-started/direct-upload/)
- [Railway CLI deployments](https://docs.railway.com/cli/deploying)
- [Railway Dockerfiles](https://docs.railway.com/builds/dockerfiles)
- [Railway health checks](https://docs.railway.com/deployments/healthchecks)
