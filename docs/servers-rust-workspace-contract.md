# Server Rust workspace contract

## Ownership and license

Rust is the default language for new server implementations unless the owning project or domain contract specifies another language. See the [repository defaults](repository-defaults.md).

`servers/Cargo.toml` owns the server Cargo workspace, independently of the root client workspace. `servers/crates/` owns shared server Rust modules. Service crates belong to their existing `servers/<service>` directories when ported. All these server files use FSL-1.1-ALv2 under the [license contract](repository-license-contract.md).

Add explicit workspace members as each service or shared crate is implemented. Each package inherits `license.workspace = true` and `publish.workspace = true`; the workspace disables registry publication. Keep server dependencies, profiles and the future `servers/Cargo.lock` separate from the root client workspace. Commit the server lockfile when the first member introduces a dependency graph. Keep server implementation modules out of root `crates/` and client dependency graphs. Protocol ownership remains in `protos/` under Apache-2.0.

## Incremental adoption

The workspace starts with no members and `servers/crates/.gitkeep` reserves the shared module directory. Cargo build and test commands require at least one member; metadata can inspect the empty workspace; no placeholder runtime or library is required. Existing Go services, the root Go module, protocol bindings and release images remain active until their individual ports are implemented.

Each port must update its project index and domain contracts, add its explicit workspace member and applicable validation and packaging integration. Run server Cargo commands from the root with `--manifest-path servers/Cargo.toml`. Root Cargo commands continue to own client packages. No server runtime is ported by this foundation change.

## Development execution

Root `pnpm dev` uses process-compose to manage each API and the frontend task group. It retains the existing workspace commands, Go Air watchers, Infisical injection, fixed loopback listeners and explicit migrations. See the [workflow contract](repository-workflow-contract.md) and [environment contract](repository-environment-contract.md). A later port changes the owning service launcher without moving shared server code across the license boundary.

### Shared build process

`server-build` is the only Cargo build owner in the process-compose session. Its helper reads workspace metadata and skips the build while there are no members. Otherwise it runs `cargo build --workspace --bins` once from `servers/`. All API processes depend on its successful completion, including when selecting a single API. Cargo is a prerequisite for root development.

The builder watches server Rust source, Cargo manifests and lockfile, SQL and protocol inputs. `cascade: true` rebuilds the workspace and restarts dependent APIs in dependency order. Generated `target/` and Go `tmp/` outputs are excluded. A failed build prevents dependent startup; the watch remains available for the next edit. Add any new build-script inputs to this watch when introducing them.

When a service is ported, its launcher must execute its compiled binary directly after environment injection. It must not invoke `cargo run`, `cargo build` or a Cargo watcher. This avoids concurrent Cargo builds against the shared target directory within the session. Go services retain Air until ported. Independent Cargo commands or other development sessions can still contend for Cargo locks; this graph does not coordinate commands outside the session.
