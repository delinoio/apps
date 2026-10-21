# DeliTools API

The DeliTools API is the public Connect service for synchronous text-array translation and schema-constrained data extraction. It runs the API and its background work in one Rust process with PostgreSQL and one running instance. The behavior contract is [the server contract](../../docs/servers-delitools-api-contract.md) and the wire contract is [the protocol contract](../../docs/protos-delitools-v1-contract.md). Server files use [FSL-1.1-ALv2](../LICENSE).

The service has two layers. The foundation provides configuration, storage and migrations, Logto and API-key authentication, key lifecycle, operator settings and request inspection, the background worker framework with 30-day retention, and the operational endpoints. The execution pipeline (`exec.rs`, `gateway.rs`, `schema.rs`, `usage.rs`, `delcore.rs`) implements `TranslateTextArray`, `ExtractDataFromText` and `AssociateGeneration`: delcore tenant authorization and billing checks for both credential types (a delcore outage fails closed), Vercel AI Gateway streaming calls with a frozen catalog price per request, the extraction schema subset and envelope, optional idempotency, durable usage delivery to delcore, Gateway usage reconciliation and public catalog price synchronization. Module ownership and every rule are in the [server contract](../../docs/servers-delitools-api-contract.md#execution-pipeline).

## Configuration

Copy `.env.example` to an uncommitted `.env` in the service working directory and replace every placeholder. Process variables take precedence over the file, including explicitly empty values. The server rejects a missing or invalid value at startup with a message that names the variable but never repeats its value, and the contract defines no defaults. `--migrate` and `check-schema` need only `DATABASE_URL`.

| Variable                                                                                                                                                                                                                                                                            | Purpose                                                                        |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------ |
| `LISTEN`, `DATABASE_URL`                                                                                                                                                                                                                                                            | HTTP listener and PostgreSQL connection                                        |
| `ISSUER`, `AUDIENCE`, `OPERATOR_ROLE`                                                                                                                                                                                                                                               | Logto token validation and the dedicated platform-operator role claim          |
| `DELCORE_URL`, `DELCORE_RESOURCE`, `LOGTO_TOKEN_URL`, `CLIENT_ID`, `CLIENT_SECRET`, `SERVICE_ID`                                                                                                                                                                                    | Delcore service identity (`SERVICE_ID` is the registered UUID v7)              |
| `GATEWAY_URL`, `GATEWAY_KEY`, `MAX_GATEWAY_RESPONSE_BYTES`                                                                                                                                                                                                                          | Vercel AI Gateway base URL including `/v1`, its API key and the response bound |
| `TIMEOUT_SECONDS`                                                                                                                                                                                                                                                                   | Outbound dependency deadline, 1 to 120                                         |
| `LOG_FORMAT`                                                                                                                                                                                                                                                                        | Optional `text` (default) or `json`                                            |
| `MAX_INPUT_BYTES`, `MAX_TRANSLATION_ITEMS`, `EXECUTION_TIMEOUT_SECONDS`, `TENANT_REQUESTS_PER_MINUTE`, `TENANT_CONCURRENCY`, `MAX_AI_RETRIES`, `PRICE_SYNC_SECONDS`, `DELIVERY_RETRY_SECONDS`, `MAX_BACKOFF_SECONDS`, `ALERT_BACKLOG`, `ALERT_EXPIRY_SECONDS`, `TRANSLATION_MODELS` | Initial execution settings (below)                                             |

All URLs must use HTTPS; loopback HTTP is accepted for local fixtures. The initial execution settings seed the database once. Afterwards only an authenticated platform operator changes them through `UpdateSettings` with a reason, and the change is audited. Changing these variables later has no effect on an initialized database. `MAX_AI_RETRIES=0` is a valid explicit value. `TRANSLATION_MODELS` is a JSON array and the only source of model eligibility.

## Database and process

Build the Rust executable from the repository root with `cargo build --locked --manifest-path servers/Cargo.toml -p delitools-api` (`protoc` is required). Run the commands from `servers/delitools-api`:

```sh
../target/debug/delitools-api --migrate       # apply explicit forward-only migrations; safe to repeat
../target/debug/delitools-api check-schema    # fail unless the schema matches this binary
../target/debug/delitools-api                 # serve
```

Normal startup never migrates. It checks the schema version and migration digests, takes a PostgreSQL session lock so only one instance serves one database, seeds the settings if the table is empty, marks requests that were still processing as unknown, and then serves. The process stops if it loses the lock session. The default development listener is `127.0.0.1:46322`.

Root `pnpm dev` starts the disposable `delitools-postgres` Compose database (`127.0.0.1:55434` by default, overridden by `DELITOOLS_POSTGRES_PORT`) and runs the built binary through the shared launcher; development launchers never run migrations.

## Operations

`/healthz` reports liveness. `/readyz` is false until the schema is compatible and the catalog holds a synchronized price. `/metrics` serves Prometheus text with bounded labels. Expose all three on a restricted network only. Every response and error carries a UUID v7 request ID in the `X-Delitools-Request-Id` header and in the typed `ErrorDetail`. Logs are structured `tracing` events that carry request IDs and bounded reasons only. They never contain inputs, schemas, results, credentials or provider bodies. Set `OTEL_EXPORTER_OTLP_ENDPOINT` for traces; apply the same 30-day retention to exported telemetry in the collector.

Records expire 30 days after creation in every state: idempotency records, attempts, requests, audit records and operational events. API keys do not expire with them.

Back up the database before an update, stop the old process, run `--migrate` and start the matching binary. Recovery is fix-forward: deploy a corrected image and, when data needs correction, a new migration. Do not roll back images or reverse migrations.

## Validation

Run these from the repository root:

```sh
cargo fmt --manifest-path servers/Cargo.toml --all --check
cargo clippy --locked --manifest-path servers/Cargo.toml -p delitools-api --all-targets -- -D warnings
cargo test --locked --manifest-path servers/Cargo.toml -p delitools-api
TEST_DATABASE_URL=postgres://... cargo test --locked --manifest-path servers/Cargo.toml -p delitools-api -- --ignored
pnpm --filter @delinoio/delitools-api ci:oci
```

PostgreSQL fixtures create and remove isolated schemas in the database named by `TEST_DATABASE_URL`; use a dedicated disposable database. The `ci:oci` command builds the image and runs disposable PostgreSQL and identity fixtures for startup, schema rejection, readiness, the instance lock and graceful shutdown. It requires Docker. Gateway, Logto and delcore are mocked, so these checks do not establish live-account or production acceptance. The integration suites are `execution` (credentials, validation, schema subset, output validation, limits, pricing availability), `reliability` (retries, cancellation, deadline, revocation during execution), `idempotency`, `usage` (price snapshots, delivery, reconciliation, association, catalog synchronization, expiry) and `privacy` (content and secrets absent from rows, logs and metrics). The delcore service client behind `CLIENT_ID` must hold the `authorize` and `usage:write` scopes and delcore must run with `DELITOOLS_ENABLED=true` and 30-day reporting windows.
