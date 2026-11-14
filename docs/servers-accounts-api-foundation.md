# servers-accounts-api foundation contract

## Scope

This contract owns the `accounts-api` component of [`delino-accounts`](project-delino-accounts.md) at `servers/accounts-api/`:

- `servers/accounts-api/cmd/accounts-api/`: process entry point.
- `servers/accounts-api/internal/config/`: environment configuration loading and validation.
- `servers/accounts-api/internal/server/`: HTTP handler, routes and middleware.
- `servers/accounts-api/.env.example`: configuration names and safe loopback defaults.
- `servers/accounts-api/package.json` and `turbo.json`: workspace task wiring.
- `servers/accounts-api/scripts/`: development launcher (`dev.mjs`) and its process-level tests.

The API server is the system of record for account and billing information. It is licensed under `FSL-1.1-ALv2` according to the [license contract](repository-license-contract.md).

## Runtime and Language

- Language: Go, in the repository root module `github.com/delinoio/mono`. The import path of the server packages is `github.com/delinoio/mono/servers/accounts-api/...`. The `go` directive in the root `go.mod` selects the toolchain version.
- Runtime: one long-running HTTP server process. It uses only the Go standard library in the current scaffold.
- Logging: `log/slog`.
- HTTP timeouts: `newServer` in `cmd/accounts-api` sets fixed constants on the `http.Server`. They are not configuration variables, so they have no `.env.example` entry and no Turbo `passThroughEnv` entry.
  - `ReadHeaderTimeout` is 10 seconds: the time a client has to send the request headers.
  - `ReadTimeout` is 30 seconds and covers the whole request read, headers and body. `net/http` clears the header deadline after the headers arrive, so without `ReadTimeout` a client that declares a body and then sends nothing would hold the connection open forever. `ReadTimeout` must stay greater than or equal to `ReadHeaderTimeout`.
  - `WriteTimeout` is 30 seconds for writing a response.
  - `IdleTimeout` is 120 seconds: a keep-alive connection that is waiting for the next request is closed after this time.
  - `WriteTimeout` limits the lifetime of every response, so a long-lived streaming RPC must lift or extend it for that handler (`http.ResponseController.SetWriteDeadline`) in the change that adds the stream.
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
- `ADDR` accepts an empty host (all interfaces), an IP address (IPv6 in brackets, with an optional zone) or an RFC 1123 hostname; configuration validation checks only the host syntax, so a hostname that does not resolve still fails when the server starts listening.
- Root `pnpm dev` and `pnpm exec turbo run dev --filter=accounts-api` pass `ADDR` and `LOG_LEVEL` from the shell to the server through Turbo `passThroughEnv` in `servers/accounts-api/turbo.json`. Add every new configuration variable to that list in the change that makes the server read it.
- Production must set `ADDR` explicitly for its deployment target. The default listens on loopback only.
- Logto settings (`planned`): the tenant endpoint and the API resource indicator. Add them to this table and to `.env.example` in the same change that adds token validation.

## Storage

The scaffold has no persistent storage. The database is not selected. Before the first persisted entity, record the database, the migration tool and the backup policy in this contract. File uploads, such as invoice documents, use Cloudflare R2 with signed URLs according to the repository defaults unless this contract documents another choice.

## Security

- Accept only Logto access tokens as API credentials. Do not accept ID tokens.
- Payment card data stays with the payment provider. The server stores provider references, not raw card numbers.
- The server never logs tokens, authorization codes, application secrets or payment details.
- Secrets, such as Logto Management API credentials and payment provider keys, follow the [environment contract](repository-environment-contract.md). They never appear in `.env.example`.

## Graceful Shutdown

- `SIGINT` or `SIGTERM` stops the listener from accepting new connections and starts a drain that waits at most 10 seconds (`shutdownTimeout`) for in-flight requests.
- The signal does not cancel request contexts. Request contexts derive from a server-lifetime context, so handlers that honour `r.Context()` finish their work normally while the server drains.
- The server cancels the server-lifetime context only after the drain ends. A handler that outlives the 10-second drain observes a cancelled request context at that point, the shutdown is reported as failed, and the process exits with a non-zero status.

## Logging

- The server writes structured JSON logs to standard output with `log/slog`. These are service logs for log aggregation, not interactive CLI output, so they do not use ANSI color.
- Startup logs the listen address and log level. Shutdown logs the signal-driven start and the result. A listener failure is never discarded, even when it races with the shutdown signal: the process exits with a non-zero status and logs the error.
- Every HTTP request produces one `http_request` log with `method`, `path`, `status`, `duration_ms` and `bytes` fields. `status` is the first committed response status (the one the client receives), `200` for an implicit body write, and informational `1xx` responses other than `101` do not count as committed. A request whose handler panics is logged too: `status` is `500` when no response status was committed before the panic and the committed status otherwise. The panic is not recovered by the logger, so `net/http` still aborts the connection.
- `net/http` internal errors, such as the report of a recovered handler panic with its stack trace and the report of a superfluous `WriteHeader` call, are logged as `ERROR` JSON records on standard output through `http.Server.ErrorLog`. They are never written as plain text to standard error, and they are not downgraded to `INFO` or dropped when `LOG_LEVEL` is `warn` or `error`. `http.ErrAbortHandler` panics stay unlogged, as `net/http` defines. The server is constructed in one place (`newServer` in `cmd/accounts-api`) so that this setting applies to every production server.
- Authentication logs (`planned`) include the Logto user ID, the API resource and the rejection reason, according to the authentication contract.

## Build and Test

Run from the repository root:

- `pnpm --filter accounts-api dev`: run the server through the development launcher `scripts/dev.mjs` (see [Development Launcher](#development-launcher)).
- `pnpm exec turbo run build --filter=accounts-api`: build the binary into `servers/accounts-api/dist/`.
- `pnpm exec turbo run lint --filter=accounts-api`: `go vet ./...`.
- `pnpm exec turbo run test --filter=accounts-api`: `go test ./...` followed by the launcher tests, `node --test "scripts/*.test.mjs"`.

The `build` script passes `-buildvcs=false`, so the binary carries no VCS stamping (`vcs.revision`, `vcs.time` and `vcs.modified` build settings). This is a workaround with the following boundaries:

- Reason: Go's default `-buildvcs=auto` discovery recognizes only a `.git` directory. In a linked Git worktree, where `.git` is a file, it walks up to an ancestor `.git` directory. When that ancestor is not a valid repository, `go build` fails with `error obtaining VCS status`; when it is a valid but unrelated repository, the binary is stamped with the wrong revision.
- Scope: the `accounts-api` `build` script and the `go build` step of the development launcher. The `lint` and `test` scripts do not stamp binaries and are unchanged.
- Removal condition: remove the flag when the Go toolchain recognizes the linked-worktree `.git` file, or when release builds move to a normal-clone job that needs VCS stamping.

Go tasks are uncached in Turbo because the Go toolchain and module cache are not declared task inputs. Remove `servers/accounts-api/dist/` after validation.

CI runs the repository Go checks in the root Go module when Go sources or Go module files change: `gofmt` check, `go vet ./...` and `go test ./...`. See the [workflow contract](repository-workflow-contract.md). CI does not build container images or deploy the server.

### Development Launcher

`scripts/dev.mjs` replaces `go run ./cmd/accounts-api`. `go run` forwards only `SIGINT` and `SIGQUIT` to the compiled server and does not tie the server's lifetime to its own, so a `SIGTERM` sent to the `go run` process (`kill`, `timeout`, a supervisor, pnpm or Turbo) left the server listening without a graceful shutdown.

- The launcher builds the server with `go build -buildvcs=false -o <temporary directory>/accounts-api ./cmd/accounts-api`, then runs the binary as its direct child. The temporary directory is created under the operating system temporary directory and removed after the last child has exited, on every exit path.
- Child processes use explicit argument arrays and `shell: false`. The working directory is `servers/accounts-api/`; standard streams and the environment, including `ADDR` and `LOG_LEVEL`, reach the server unchanged. The server configuration, its logs and `/healthz` are unchanged, and the production binary is built by the `build` script, not by the launcher.
- The launcher forwards `SIGINT` and `SIGTERM` to the active child (the build or the server) and writes an `accounts_api_dev_signal` JSON event to standard error. It does not handle `SIGHUP`. It waits for the child to exit, so the server drains and logs `server_shutdown_started` and `server_shutdown_completed` before the launcher completes.
- A signal received during the build stops the launcher after the build exits, even when the build succeeded. The server never starts after a termination request.
- Exit status: the status of the failed build, the server's own status (non-zero for an invalid configuration or an occupied port), or `128` plus the signal number after a forwarded signal (`130` for `SIGINT`, `143` for `SIGTERM`).
- The launcher tests build and run the real server with a free loopback port. They cover a `SIGTERM` to the launcher alone, a process-group `SIGINT`, an occupied port, an invalid `LOG_LEVEL`, a build failure and a `SIGTERM` during the build. They use POSIX signals and are skipped on Windows.
- The `go build` step carries the same `-buildvcs=false` workaround and removal condition as the `build` script above.

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
