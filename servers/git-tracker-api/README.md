# Git Tracker API

Git Tracker stores exact numeric measurements for Git commits. The web application, GitHub Action and Connect schema live in this repository alongside the server. Server files use [FSL-1.1-ALv2](../LICENSE).

## Configuration

Copy the names in `.env.example` into the operator's environment. No environment file is loaded automatically. Set a PostgreSQL URL, positive audit retention duration, GitHub App ID/private-key file/webhook secret, Delcore endpoint and Logto M2M credentials. The M2M application must be registered in Delcore with the `authorize` scope and `view`, `edit`, `manage` capabilities. Grant these capabilities through existing Delcore groups. Connection additionally requires Delcore Owner/Admin authority. There is no separate membership system.

Use a GitHub App with selected repository access and repository metadata, contents, pull requests and issue-comment permissions. Subscribe to repository, installation and installation-repositories events at `/github/events`. The App private key stays server-side. Connection verifies an administrator's GitHub access token; the token is never retained. Actions credentials use issuer `https://token.actions.githubusercontent.com` and audience `https://git-tracker.api.delino.io`.

The web origin is an exact CORS allowlist entry. The default listener is `127.0.0.1:46321`. OTLP settings follow the OpenTelemetry environment convention. The service validates required configuration at startup. Do not use test fixture credentials for real integrations.

## Database and process

Run `go run ./servers/git-tracker-api/cmd/git-tracker-api migrate` once against an empty Git Tracker database. Migration errors stop the command. Start the process with `go run ./servers/git-tracker-api/cmd/git-tracker-api`. The process handles graceful shutdown and runs interruption, finalized-space deletion and audit retention maintenance every minute.

Repository rows serialize atomic uploads, completion, deletion and comment publication across service instances. JSONB stores decimal strings and frozen references; comparison uses exact rational arithmetic. Tokens are stored only as SHA-256 digests. Deleting a referenced baseline does not preserve its measurements in comparison caches.

`/healthz` reports liveness; `/readyz` checks database connectivity and schema presence; `/metrics` exposes bounded Prometheus counters. The process emits structured logs and OpenTelemetry HTTP traces. Responses include correlation identifiers and `Cache-Control: no-store`. Never include raw measurements, tokens or provider payloads in telemetry.

## Alert rules

Generate Prometheus rules with `go run ./servers/git-tracker-api/cmd/git-tracker-api alerts`. For each of `API_ERRORS`, `PERSISTENCE`, `GITHUB` and `INTERRUPTED`, supply `ALERT_<name>_THRESHOLD`, `ALERT_<name>_WINDOW` and `ALERT_<name>_FOR`. Thresholds are nonnegative numbers. Windows and hold durations are positive Prometheus durations ending in `s`, `m`, `h` or `d`. API errors use a rate ratio; other rules use event counts over the observation window. Operators choose all values and connect receiving routes through Alertmanager. The service sends no alert messages.

## Validation

Run `GOWORK=off go test ./...` and `GOWORK=off go vet ./...` from the root. Set `TEST_DATABASE_URL` to a dedicated disposable PostgreSQL database and run `GOWORK=off go test -race ./servers/git-tracker-api/...`. Integration tests reset Git Tracker tables in that test database. CI runs these tests on an isolated PostgreSQL container. External integrations are mocked in automated tests; they do not establish live-account or deployment acceptance.

Operator logs use ANSI color by default. Set `NO_COLOR=1` for uncolored structured key/value output.
