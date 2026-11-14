# servers-accounts-api foundation contract

## Scope

This contract owns the `accounts-api` component of [`delino-accounts`](project-delino-accounts.md) at `servers/accounts-api/`:

- `servers/accounts-api/cmd/accounts-api/`: process entry point.
- `servers/accounts-api/internal/config/`: environment configuration loading and validation.
- `servers/accounts-api/internal/server/`: HTTP handler, routes and middleware.
- `servers/accounts-api/.env.example`: configuration names and safe loopback defaults.
- `servers/accounts-api/package.json` and `turbo.json`: workspace task wiring.

The API server is the system of record for account and billing information. It is licensed under `FSL-1.1-ALv2` according to the [license contract](repository-license-contract.md).

## Runtime and Language

- Language: Go, in the repository root module `github.com/delinoio/mono`. The import path of the server packages is `github.com/delinoio/mono/servers/accounts-api/...`. The `go` directive in the root `go.mod` selects the toolchain version.
- Runtime: one long-running HTTP server process. It uses only the Go standard library in the current scaffold.
- Logging: `log/slog`.
- Deployment target: not selected. Record the target, its configuration source and its TLS termination in this contract before the first deployment.

## Users and Operators

- The [`delino-accounts-web`](apps-delino-accounts-foundation.md) app, which calls the API on behalf of signed-in users.
- Other Delino services. The repository rules require every service to use this API for shared account data and for billing, including billable usage or charge reports (`planned`). Record each consuming service and the RPCs that it uses here before it calls the API.
- Operators who deploy and monitor the server.

## Interfaces and Contracts

- Production origin: `https://accounts.api.delino.io`.
- Operational endpoints:

| Endpoint ID | Method and path | Authentication | Response                                                                |
| ----------- | --------------- | -------------- | ----------------------------------------------------------------------- |
| `healthz`   | `GET /healthz`  | none           | `200` with `{"status":"ok"}` while the process can serve HTTP requests. |

- Business APIs (`planned`): Connect RPC services generated from `protos/delino/accounts/v1/`. Planned services:

| Service ID       | Responsibility                                                                        |
| ---------------- | ------------------------------------------------------------------------------------- |
| `AccountService` | Read and update the profile and preferences of the signed-in Logto user.              |
| `BillingService` | Manage payment methods, billing contacts, balances, credits and invoices of the user. |

- Authentication (`planned` with the first business API): the server is the Logto API resource `https://accounts.api.delino.io`. It validates every access token according to the [authentication contract](repository-auth-contract.md) and checks the scopes that each RPC requires. Record the scope names in this table when the RPCs are added.
- Browser access (`planned` with the first business API): CORS allows only the `https://accounts.delino.io` origin in production and the loopback development origin locally.
- Identifiers: persisted entities use UUID v7. User-owned records reference the Logto user ID (`sub`).
- Money: all amounts are `int64` micro-USD values with a `_usd_micros` suffix, according to the [billing contract](repository-billing-contract.md). Record the payment provider conversion and its rounding direction here when the provider is selected.

## Configuration

| Variable    | Required | Default          | Values                           |
| ----------- | -------- | ---------------- | -------------------------------- |
| `ADDR`      | no       | `127.0.0.1:8100` | `host:port` listen address       |
| `LOG_LEVEL` | no       | `info`           | `debug`, `info`, `warn`, `error` |

- The server validates the configuration at startup and exits with a non-zero status when a value is not valid.
- Production must set `ADDR` explicitly for its deployment target. The default listens on loopback only.
- Logto settings (`planned`): the tenant endpoint and the API resource indicator. Add them to this table and to `.env.example` in the same change that adds token validation.

## Storage

The scaffold has no persistent storage. The database is not selected. Before the first persisted entity, record the database, the migration tool and the backup policy in this contract. File uploads, such as invoice documents, use Cloudflare R2 with signed URLs according to the repository defaults unless this contract documents another choice.

## Security

- Accept only Logto access tokens as API credentials. Do not accept ID tokens.
- Payment card data stays with the payment provider. The server stores provider references, not raw card numbers.
- The server never logs tokens, authorization codes, application secrets or payment details.
- Secrets, such as Logto Management API credentials and payment provider keys, follow the [environment contract](repository-environment-contract.md). They never appear in `.env.example`.

## Logging

- The server writes structured JSON logs to standard output with `log/slog`. These are service logs for log aggregation, not interactive CLI output, so they do not use ANSI color.
- Startup logs the listen address and log level. Shutdown logs the signal-driven start and the result.
- Every HTTP request produces one `http_request` log with `method`, `path`, `status`, `duration_ms` and `bytes` fields.
- Authentication logs (`planned`) include the Logto user ID, the API resource and the rejection reason, according to the authentication contract.

## Build and Test

Run from the repository root:

- `pnpm --filter accounts-api dev`: run the server with `go run`.
- `pnpm exec turbo run build --filter=accounts-api`: build the binary into `servers/accounts-api/dist/`.
- `pnpm exec turbo run lint --filter=accounts-api`: `go vet ./...`.
- `pnpm exec turbo run test --filter=accounts-api`: `go test ./...`.

Go tasks are uncached in Turbo because the Go toolchain and module cache are not declared task inputs. Remove `servers/accounts-api/dist/` after validation.

CI runs the repository Go checks in the root Go module when Go sources or Go module files change: `gofmt` check, `go vet ./...` and `go test ./...`. See the [workflow contract](repository-workflow-contract.md). CI does not build container images or deploy the server.

## Dependencies and Integrations

- Upstream: Logto shared tenant (`planned`), payment provider (not selected), database (not selected).
- Downstream: [`delino-accounts-web`](apps-delino-accounts-foundation.md).
- Protocol schemas: `protos/delino/accounts/` (`planned`).

## Change Triggers

- Update [project-delino-accounts.md](project-delino-accounts.md) when the origin, component IDs, deployment target or open decisions change.
- Update [apps-delino-accounts-foundation.md](apps-delino-accounts-foundation.md) when an API capability or allowed browser origin changes.
- Add a `protos-delino-accounts-*` contract with the first Connect RPC schema.
- Update [repository-workflow-contract.md](repository-workflow-contract.md) when Go validation or deployment workflows change.
- Update `servers/AGENTS.md` only under the [instruction-update policy](README.md#instruction-update-policy).

## References

- [project-delino-accounts.md](project-delino-accounts.md)
- [repository-defaults.md](repository-defaults.md)
- [repository-auth-contract.md](repository-auth-contract.md)
- [repository-billing-contract.md](repository-billing-contract.md)
- [repository-environment-contract.md](repository-environment-contract.md)
- [repository-license-contract.md](repository-license-contract.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [Connect for Go](https://connectrpc.com/docs/go/getting-started)
