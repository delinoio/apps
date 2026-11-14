# apps-delino-accounts foundation contract

## Scope

This contract owns the `delino-accounts-web` component of [`delino-accounts`](project-delino-accounts.md) at `apps/delino-accounts/`:

- `apps/delino-accounts/rsbuild.config.ts`: build and development server configuration.
- `apps/delino-accounts/src/`: application source, entry point and unit tests.
- `apps/delino-accounts/public/`: static files copied to the build root, including Cloudflare Pages `_headers` and `_redirects`.
- `apps/delino-accounts/turbo.json`: package-local Turbo task configuration.

## Runtime and Language

- Toolchain: Rsbuild `@rsbuild/core` 2.x with `@rsbuild/plugin-react`, React 19.
- Language: TypeScript.
- Test runner: Rstest `@rstest/core`.
- Runtime: Node.js from root `.nvmrc` for builds and tests. Production output is a static single-page application with no server runtime.
- Deployment target: Cloudflare Pages, production domain `accounts.delino.io`.
- Deviation from repository defaults: the app uses Rsbuild instead of Rspress because it is an authenticated application. See the [project index](project-delino-accounts.md).

## Users and Operators

- Signed-in Delino users who manage their account and billing information.
- Maintainers who develop and deploy the web app.

## Interfaces and Contracts

- Production origin: `https://accounts.delino.io`. The asset prefix is `/`.
- Content language: English (`lang="en"`).

| Route ID  | Path       | Status    | Purpose                                                            |
| --------- | ---------- | --------- | ------------------------------------------------------------------ |
| `home`    | `/`        | available | Entry page that introduces the account and billing sections.       |
| `account` | `/account` | planned   | Profile and preferences attached to the Logto user.                |
| `billing` | `/billing` | planned   | Payment methods, billing contacts, balances, credits and invoices. |

- `public/_redirects` rewrites every unknown path to `/index.html` with status `200`, so client-side routes load on direct navigation.
- `public/_headers` sets Cloudflare Pages response headers: `X-Content-Type-Options: nosniff`, `Referrer-Policy: strict-origin-when-cross-origin` and `X-Frame-Options: DENY`.
- API origin: `src/config.ts` resolves the API server origin from the build-time variable `PUBLIC_DELINO_ACCOUNTS_API_ORIGIN`. An unset or empty value selects `https://accounts.api.delino.io`. The value must be an `http` or `https` origin with no path, query or fragment; the build fails for other values.
- API calls (`planned`): Connect RPC through `@connectrpc/connect-query` and React Query. The app must not call the API through Tauri-specific or ad-hoc bindings.
- Authentication (`planned`): the app is registered as a Logto single-page application in the shared tenant and requests access tokens for the API resource `https://accounts.api.delino.io`. Sign-in redirect URIs use the `https://accounts.delino.io` origin in production and the loopback development origin locally.
- Monetary values from the API are `bigint` micro-USD amounts. The app formats them for display only and never converts them to `number` for arithmetic.

## Storage

The app has no server-side storage. All account and billing state belongs to the [API server](servers-delino-accounts-api-foundation.md). Browser storage is limited to the Logto SDK session data and per-user display preferences. Static assets larger than 512 KiB require Git LFS according to the repository workflow contract.

## Security

- Do not embed secrets in the build. Every `PUBLIC_` variable is public and is inlined into the client bundle.
- Logto application IDs and the tenant endpoint are configuration, not secrets. The app must not use a Logto application secret.
- Billing input that contains payment card data must go directly to the payment provider's hosted fields. The app and the API server must not receive raw card numbers.
- The page must not be framed (`X-Frame-Options: DENY`).

## Logging

The app has no server logs. Build diagnostics come from the Rsbuild CLI output in local runs and CI logs. Cloudflare Pages owns request analytics. Client-side logs must not contain access tokens, ID tokens or payment details.

## Build and Test

Run from the repository root:

- `pnpm install --frozen-lockfile`
- `pnpm --filter delino-accounts dev`: development server at `http://127.0.0.1:3100`. The server fails when the port is in use.
- `pnpm exec turbo run build --filter=delino-accounts`: production build into `apps/delino-accounts/dist/`.
- `pnpm exec turbo run typecheck --filter=delino-accounts`: TypeScript type check.
- `pnpm exec turbo run test --filter=delino-accounts`: Rstest unit tests.
- `pnpm --filter delino-accounts preview`: serve the production build locally.

`turbo.json` declares `PUBLIC_DELINO_ACCOUNTS_API_ORIGIN` and local `.env*` files as build inputs. Remove `apps/delino-accounts/dist/` after validation. CI does not build, test or deploy this app yet; add those checks to the workflow and its planner when the app enters CI.

## Dependencies and Integrations

- `@rsbuild/core`, `@rsbuild/plugin-react`, `react`, `react-dom` and `@rstest/core`, pinned in `apps/delino-accounts/package.json`.
- [`delino-accounts-api`](servers-delino-accounts-api-foundation.md) at `https://accounts.api.delino.io`.
- Logto shared tenant, according to the [authentication contract](repository-auth-contract.md).
- Cloudflare Pages: static hosting for `accounts.delino.io`. Configure the Pages project with build command `pnpm exec turbo run build --filter=delino-accounts` and output directory `apps/delino-accounts/dist`.

## Change Triggers

- Update [project-delino-accounts.md](project-delino-accounts.md) when paths, the production origin or the web-to-API boundary change.
- Update [servers-delino-accounts-api-foundation.md](servers-delino-accounts-api-foundation.md) when the app needs a new API capability or a new allowed browser origin.
- Update [repository-workflow-contract.md](repository-workflow-contract.md) when this app enters CI or a deployment workflow.
- Update `apps/AGENTS.md` only under the [instruction-update policy](README.md#instruction-update-policy).

## References

- [project-delino-accounts.md](project-delino-accounts.md)
- [repository-defaults.md](repository-defaults.md)
- [repository-auth-contract.md](repository-auth-contract.md)
- [repository-billing-contract.md](repository-billing-contract.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [Rsbuild documentation](https://rsbuild.rs/)
- [Rstest documentation](https://rstest.rs/)
- [Connect-Query](https://github.com/connectrpc/connect-query-es)
