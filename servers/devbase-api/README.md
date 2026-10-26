# DevBase API

The DevBase API is a free, project-scoped directory of end users that developers manage for their own applications. Developers keep Delino tenants and projects; their applications use project API keys to read and write the end users of one project. End users never log in to this service. The behavior contract is [the server contract](../../docs/servers-devbase-api-contract.md) and the wire contract is [the protocol contract](../../docs/protos-devbase-v1-contract.md). Server files use [FSL-1.1-ALv2](../LICENSE).

## Configuration

Copy the names in `.env.example` into the operator's environment. No environment file is loaded automatically except by the development watcher. Set a PostgreSQL URL and this service's own Delcore consumer identity: the Delcore base URL, the Logto resource, the Logto token endpoint and the machine-to-machine client ID and secret. Register the application in Delcore as a consumer service with the `authorize` scope. Limits and timeouts have documented defaults and can be overridden. The server validates configuration at startup and refuses missing, non-positive or contradictory values.

## Database and process

Build the Rust executable from the repository root with `cargo build --locked --manifest-path servers/Cargo.toml -p devbase-api`. Cargo requires `protoc`. Root development builds all three servers once through Turborepo; service-local development runs the already built binary.

```sh
./servers/target/debug/devbase-api migrate        # apply explicit migrations; safe to repeat
./servers/target/debug/devbase-api check-schema   # fail unless the schema matches this binary
./servers/target/debug/devbase-api                # serve
```

Normal startup never migrates. It checks the schema version and digests, then takes a PostgreSQL session lock so only one instance serves one database. `/healthz` reports liveness, `/readyz` checks schema compatibility and `/metrics` exposes Prometheus counters and histograms with bounded labels. The default listener is `127.0.0.1:46317`. `WEB_ORIGIN` names the one exact browser origin, the DevBase console, that may call the API; requests from any other origin are rejected, and requests without an `Origin` header are unaffected.

Recovery is fix-forward: deploy a corrected image and, when data needs correction, a new migration. Take a database backup before migrating. Do not roll back images or reverse migrations.

## Validation

Run these from the repository root:

```sh
cargo test --locked --manifest-path servers/Cargo.toml -p devbase-api
TEST_DATABASE_URL=postgres://... cargo test --locked --manifest-path servers/Cargo.toml -p devbase-api --test postgres -- --ignored
pnpm --filter @servers/devbase-api ci:oci
```

Rust PostgreSQL fixtures create and remove isolated schemas in the database named by `TEST_DATABASE_URL`; use a dedicated disposable database. The `ci:oci` command builds the image only. Delcore is mocked, so these checks do not establish live-account or production acceptance. Operator logs use ANSI color by default; set `NO_COLOR=1` for plain structured key/value output, or `LOG_FORMAT=json` for one JSON object per record.

Rust checks: `cargo test --locked --manifest-path servers/Cargo.toml -p devbase-api`. Set a disposable `TEST_DATABASE_URL` and add `--test postgres -- --ignored` for Rust PostgreSQL fixtures, which use an isolated schema. Development mode loads an optional service-local `.env` without overriding injected variables. Production and operator commands do not load that file.
