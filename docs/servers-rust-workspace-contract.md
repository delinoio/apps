# Server Rust workspace contract

## Ownership and license

`servers/Cargo.toml` owns the server Cargo workspace, independently of the root client workspace. `servers/crates/` owns shared server Rust modules. Service crates belong to their existing `servers/<service>` directories when ported. All these server files use FSL-1.1-ALv2 under the [license contract](repository-license-contract.md).

Add explicit workspace members as each service or shared crate is implemented. Each package inherits `license.workspace = true` and `publish.workspace = true`; the workspace disables registry publication. Keep server dependencies, profiles and `servers/Cargo.lock` separate from the root client workspace. Commit the server lockfile when the first member introduces a dependency graph. Keep server implementation modules out of root `crates/` and client dependency graphs. Protocol ownership remains in `protos/` under Apache-2.0.

## Incremental adoption

The workspace contains `devbase-api`, `git-tracker-api`, `crates/server-common` and `crates/delino-protos`. Shared authentication, configuration, HTTP policy, rate limits and telemetry belong to `server-common`. Delcore retains its Go runtime. The root Go module and generated Go clients remain available for consumers and compatibility tests. The existing Go service sources are retained as test oracles; development and OCI entrypoints for DevBase and Git Tracker use Rust only.

Each port must update its project index and domain contracts, add its explicit workspace member and applicable validation and packaging integration. Run server Cargo commands from the root with `--manifest-path servers/Cargo.toml`. Root Cargo commands continue to own client packages. Rust protocol bindings are owned by the server workspace package `servers/crates/delino-protos`. Its build script generates bindings from the existing schemas into Cargo output; no schema or generated source is copied into server source directories. Install `protoc` before compiling.

## Development execution

Root `pnpm dev` uses process-compose to manage each API and the frontend task group. It retains workspace commands, Infisical injection, fixed loopback listeners and explicit migrations. Delcore retains Air; the two Rust launchers execute the binaries built by `server-build`. See the [workflow contract](repository-workflow-contract.md) and [environment contract](repository-environment-contract.md). For a service-local launch, first run `cargo build --manifest-path servers/Cargo.toml --workspace --bins` from the root. The launcher does not build or watch. Both Rust API Turbo tasks pass through `CARGO_TARGET_DIR` so they use the same output directory as the shared builder. Its internal `--dev-env` flag loads the service-local `.env` without overriding injected or explicitly empty process variables.

### Shared build process

`server-build` is the only Cargo build owner in the process-compose session. Its helper reads workspace metadata and skips the build while there are no members. Otherwise it runs `cargo build --workspace --bins` once from `servers/`. All API processes depend on its successful completion, including when selecting a single API. Cargo is a prerequisite for root development.

The builder watches server Rust source, Cargo manifests and lockfile, SQL and protocol inputs. `cascade: true` rebuilds the workspace and restarts dependent APIs in dependency order. Generated `target/` and Go `tmp/` outputs are excluded. A failed build prevents dependent startup; the watch remains available for the next edit. Add any new build-script inputs to this watch when introducing them.

When a service is ported, its launcher must execute its compiled binary directly after environment injection. It must not invoke `cargo run`, `cargo build` or a Cargo watcher. This avoids concurrent Cargo builds against the shared target directory within the session. Go services retain Air until ported. Independent Cargo commands or other development sessions can still contend for Cargo locks; this graph does not coordinate commands outside the session.

## Validation and packaging

Run `cargo fmt --manifest-path servers/Cargo.toml --all --check`, `cargo clippy --locked --manifest-path servers/Cargo.toml --workspace --all-targets -- -D warnings` and `cargo test --locked --manifest-path servers/Cargo.toml`. Run the PostgreSQL tests with a disposable `TEST_DATABASE_URL` and `cargo test --locked --manifest-path servers/Cargo.toml --workspace --test postgres -- --ignored`. Each Rust integration test creates and removes its own schema. Existing Go oracle tests retain their original database-reset requirements.

The owning Linux API CI jobs run Rust checks, PostgreSQL fixtures and OCI builds. Changes in shared server crates select both Rust API jobs. Root client Cargo validation remains separate. Images compile the locked server workspace and package the Rust executable with server and protocol license notices. Never include local environment files, Cargo targets or credentials in the Docker context.

## HTTP transport limits

The shared transport bounds header parsing before Axum middleware runs, idle socket reads and stalled writes. Header limits are 5 seconds for DevBase and 10 seconds for Git Tracker; idle reads are limited to 60 seconds. Stalled writes are limited to the DevBase request timeout plus 5 seconds, or 60 seconds for Git Tracker. Request middleware separately bounds complete body decoding and handler execution. Shutdown stops accepting connections and drains existing connections within the service shutdown deadline.
