# DeliTools API

DeliTools exposes synchronous Connect RPC translation and structured extraction. It is a separate service from the DeliTools application. This distribution uses FSL-1.1-ALv2; included protocol code retains Apache-2.0.

## Configure and start

Run PostgreSQL and register the `delitools` service in delcore. Give its M2M identity `authorize` and `usage:write` scopes, register `translator` and `extractor` capabilities/meters, and configure tenant entitlements. Use the shared Delino resource audience for user access tokens. Assign the dedicated DeliTools platform-operator role only to platform operators.

Create operator-owned configuration from `config.example.json` and supply the environment names in `.env.example`. All example numeric values are illustrative operator choices, not product defaults. Keep credentials and actual configuration outside Git. User tokens and API keys must not appear in logs.

From the repository root:

```sh
go build -o /tmp/delitools-api ./servers/delitools-api/cmd/delitools-api
/tmp/delitools-api -config /path/to/operator-config.json -migrate
/tmp/delitools-api -config /path/to/operator-config.json
```

Migrations are explicit; normal startup rejects an absent or incompatible schema. Run one instance. The example listens on `127.0.0.1:46322` and fails on a port conflict. Use an operator-selected container bind address when publishing an OCI port. API startup is separate from workspace frontend development.

`/healthz` checks the process; `/readyz` checks database schema access. `/metrics` exposes Prometheus measurements. Restrict diagnostics to the operator network. An optional OTLP exporter uses the standard `OTEL_EXPORTER_OTLP_ENDPOINT` or `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT`. Console logs are structured text with ANSI color; set `NO_COLOR=1` to disable color or pass `-log-json` for uncolored JSON.

The initial settings are required even when a database already exists. After initialization, authorized `UpdateSettings` calls own stored execution settings. Every mutation requires a reason and audit record. Operator configuration changes to dependency URLs, credentials and transport bounds require a restart. Configure 30-day retention for exported logs and traces.

Back up the database before an update. Stop the old instance, apply the new forward migrations and start the matching binary. Fix application/schema defects forward. Do not restore an older image against a changed schema or run reverse migrations. Production infrastructure and deployment remain operator-owned.

## Build the image

```sh
docker build -f servers/delitools-api/Dockerfile -t delitools-api:local .
```

The image runs as a non-root user. Mount the operator configuration read-only and inject credentials at runtime. Run `-migrate` separately before serving traffic. Do not bake credentials into the image.

## Validate

```sh
go test ./...
go vet ./servers/...
go test -race ./servers/...
go build ./servers/delitools-api/cmd/...
DELITOOLS_TEST_DATABASE_URL='postgres://postgres:fixture-only@127.0.0.1:5432/delitools_test?sslmode=disable' go test -race -tags integration ./servers/delitools-api/...
pnpm --filter @delinoio/delitools-api ci:oci
```

Use a disposable database. Integration tests create and remove isolated schemas and use mocked authorities/providers. OCI tests create disposable containers and verify migration, startup, readiness, metrics, single-instance enforcement and schema/configuration rejection. Automated fixtures and image checks do not establish production deployment or live provider-account acceptance.
