# Server Rust workspace contract

## Ownership and license

Rust is the default language for new server implementations unless the owning project or domain contract specifies another language. See the [repository defaults](repository-defaults.md).

`servers/Cargo.toml` owns the server Cargo workspace, independently of the root client workspace. `servers/crates/` owns shared server Rust modules. Service crates belong to their existing `servers/<service>` directories when ported. All these server files use FSL-1.1-ALv2 under the [license contract](repository-license-contract.md).

Add explicit workspace members as each service or shared crate is implemented. Each package inherits `license.workspace = true` and `publish.workspace = true`; the workspace disables registry publication. Keep server dependencies, profiles and `servers/Cargo.lock` separate from the root client workspace. Commit the server lockfile when the first member introduces a dependency graph. Keep server implementation modules out of root `crates/` and client dependency graphs. Protocol ownership remains in `protos/` under Apache-2.0.

## Incremental adoption

The workspace contains `delcore-api`, `devbase-api`, `git-tracker-api`, `delitools-api`, `crates/server-common` and `crates/delino-protos`. Shared authentication, configuration, HTTP policy, rate limits and telemetry belong to `server-common`. The root Go module and generated Go clients remain available for protocol consumers. Delcore, DevBase and Git Tracker keep only Rust runtime source and tests; their Go service implementations, Go test oracles and Air configurations are removed. SQL migrations remain canonical inputs to the Rust binaries. DeliTools has never had a Go implementation: it is a Rust service from the start and follows the same layout.

Each port must update its project index and domain contracts, add its explicit workspace member and applicable validation and packaging integration. Run server Cargo commands from the root with `--manifest-path servers/Cargo.toml`. Root Cargo commands continue to own client packages. Rust protocol bindings are owned by the server workspace package `servers/crates/delino-protos`. Its build script generates bindings from the existing schemas into Cargo output; no schema or generated source is copied into server source directories. Install `protoc` before compiling.

## Development execution

Root `pnpm dev` uses `pnpm exec turbo run dev` to manage the APIs and frontends. It retains workspace commands, Infisical injection, fixed loopback listeners and explicit migrations. All four Rust launchers execute the binaries built by `//#server-build`. See the [workflow contract](repository-workflow-contract.md) and [environment contract](repository-environment-contract.md). For a service-local launch, first run `cargo build --manifest-path servers/Cargo.toml --workspace --bins` from the root. The launcher does not build or watch. All Rust API Turbo tasks pass through `CARGO_TARGET_DIR` so they use the same output directory as the shared builder. The internal `--dev-env` flag loads the service-local `.env` for DevBase, Git Tracker and DeliTools. Delcore accepts this flag and preserves optional working-directory `.env` loading for every invocation. None overrides injected or explicitly empty process variables.

### Shared build process

The root `//#server-build` Turbo task is the only Cargo build owner in the development session. Its helper reads workspace metadata and skips the build while there are no members. Otherwise it runs `cargo build --workspace --bins` once from `servers/`. All API processes depend on its successful completion, including when selecting a single API. Cargo is a prerequisite for root development.

The shared task declares server Rust source, Cargo manifests and lockfile, SQL, protocol inputs and the build helper as inputs. Each API task depends on `//#server-build`, so the initial build completes before API startup. Turbo does not watch for changes or automatically restart APIs. Stop and rerun root `pnpm dev` to rebuild after server changes or apply catalog YAML edits. A failed build prevents dependent startup and ends the run; correct the failure and rerun the command. Generated `target/` and Go `tmp/` outputs are ignored through Git exclusions. Add any new build-script inputs to the shared task in `turbo.json` when introducing them.

When a service is ported, its launcher must execute its compiled binary directly after environment injection. It must not invoke `cargo run`, `cargo build` or a Cargo watcher. This avoids concurrent Cargo builds against the shared target directory within the session. Independent Cargo commands or other development sessions can still contend for Cargo locks; this graph does not coordinate commands outside the session.

## Validation and packaging

Run `cargo fmt --manifest-path servers/Cargo.toml --all --check`, `cargo clippy --locked --manifest-path servers/Cargo.toml --workspace --all-targets -- -D warnings` and `cargo test --locked --manifest-path servers/Cargo.toml`. Run the PostgreSQL tests with a disposable `TEST_DATABASE_URL` and `cargo test --locked --manifest-path servers/Cargo.toml --workspace -- --ignored`. Each Rust integration test creates and removes its own schema.

The Linux `rust-servers-clippy` CI matrix runs service-specific Clippy with all targets and warnings denied. The independent `rust-servers` matrix runs unit tests and PostgreSQL fixtures. Both jobs use the same affected-service selection and retain separate validation evidence. Neither installs Go. Changes in shared server crates select all four Rust API entries. The planner emits a `delitools-api` entry with its own disposable PostgreSQL database for the matrices; protocol changes under `protos/` select it as they select the other services. Ordinary CI omits OCI tests and image builds; explicit local packaging checks and release image publication remain separate. Root client Cargo validation remains separate. Images compile the locked server workspace and package the Rust executable with server and protocol license notices. Never include local environment files, Cargo targets or credentials in the Docker context.

## HTTP transport limits

The shared transport bounds header parsing before Axum middleware runs, idle socket reads and stalled writes. Header limits are 5 seconds for DevBase and 10 seconds for Git Tracker; idle reads are limited to 60 seconds. Stalled writes are limited to the DevBase request timeout plus 5 seconds, or 60 seconds for Git Tracker. DeliTools uses the DevBase header limit, and its stalled-write limit is its execution timeout plus 5 seconds. Request middleware separately bounds complete body decoding and handler execution. Shutdown stops accepting connections and drains existing connections within the service shutdown deadline.
