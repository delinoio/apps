# Project: delino-accounts

## Goal

Provide one place where Delino users manage their account information and their billing information. Account information includes the profile and preferences that are attached to the shared Logto identity. Billing information includes payment methods, billing contacts, balances, credits and invoices.

## Project ID

`delino-accounts`

## Domain Ownership Map

- `apps/delino-accounts/`: Rsbuild React web app served at `https://accounts.delino.io`.
- `servers/accounts-api/`: Go API server served at `https://accounts.api.delino.io`.
- `protos/delino/accounts/` (`planned`): Connect RPC schemas for the account and billing APIs.

The project has two deployables. The web app is a static single-page application with no server runtime. The API server owns all account and billing state. The project has no `cmds`, `crates` or `packages` paths.

## Domain Contract Documents

- [apps-delino-accounts-foundation.md](apps-delino-accounts-foundation.md)
- [servers-accounts-api-foundation.md](servers-accounts-api-foundation.md)

## Cross-Domain Invariants

- Component IDs:

| Component ID          | Path                    | Production origin                |
| --------------------- | ----------------------- | -------------------------------- |
| `delino-accounts-web` | `apps/delino-accounts/` | `https://accounts.delino.io`     |
| `accounts-api`        | `servers/accounts-api/` | `https://accounts.api.delino.io` |

- The web app calls the API server only through its absolute origin. The web app reads the origin from `PUBLIC_DELINO_ACCOUNTS_API_ORIGIN` and uses `https://accounts.api.delino.io` when the value is not set. The web app must not import source from `servers/`.
- Business flows between the web app and the API server use Connect RPC. The schemas will live in `protos/delino/accounts/`. The web app will use `@connectrpc/connect-query` with React Query for server state. Plain HTTP endpoints are limited to operational endpoints such as health checks.
- Authentication follows the [repository authentication contract](repository-auth-contract.md). The web app is a Logto single-page application. The API server is a Logto API resource with the indicator `https://accounts.api.delino.io` and validates Logto access tokens. The Logto user ID (`sub`) is the owner key of all account and billing records.
- Billing amounts follow the [repository billing contract](repository-billing-contract.md): USD only, signed 64-bit integers in micro-USD with a `_usd_micros` suffix.
- New persisted entities use UUID v7 identifiers.
- `delino-accounts` is the shared account management and billing system for every Delino service, according to the [authentication](repository-auth-contract.md#account-management) and [billing](repository-billing-contract.md#billing-ownership) contracts. Other services link users to `https://accounts.delino.io` and call `https://accounts.api.delino.io`. They do not keep their own account management, payment-provider integration or balances.
- Deviation from repository defaults: the web app uses Rsbuild instead of Rspress because it is an authenticated application, not a documentation or marketing site. It still deploys to Cloudflare Pages as static output.
- Open decisions: the API server deployment target, the persistent database and the payment provider are not selected yet. Record each decision in the API server contract before the implementation that depends on it.

## Change Policy

- Update [apps-delino-accounts-foundation.md](apps-delino-accounts-foundation.md), [servers-accounts-api-foundation.md](servers-accounts-api-foundation.md) and this index together when origins, component IDs or the web-to-API boundary change.
- Add a `protos-delino-accounts-*` contract and change the `planned` protos path to an actual path in the same change that adds the first Connect RPC schema.
- Update the [documentation catalog](README.md) when contracts are added, renamed or removed.
- Update `apps/AGENTS.md` and `servers/AGENTS.md` only when development procedures, directory ownership or development rules change; follow the [instruction-update policy](README.md#instruction-update-policy).
- Record implementation status, validation results and unresolved limits in pull requests, Linear issues and CI logs/artifacts.

## References

- [repository-defaults.md](repository-defaults.md)
- [repository-auth-contract.md](repository-auth-contract.md)
- [repository-billing-contract.md](repository-billing-contract.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [repository-environment-contract.md](repository-environment-contract.md)
- [repository-license-contract.md](repository-license-contract.md)
- [project-template.md](project-template.md)
