# Server Rust workspace contract

## Ownership and license

`servers/Cargo.toml` owns the server Cargo workspace, independently of the root client workspace. `servers/crates/` owns shared server Rust modules. Service crates belong to their existing `servers/<service>` directories when ported. All these server files use FSL-1.1-ALv2 under the [license contract](repository-license-contract.md).

Add explicit workspace members as each service or shared crate is implemented. Each package inherits `license.workspace = true` and `publish.workspace = true`; the workspace disables registry publication. Keep server dependencies, profiles and the future `servers/Cargo.lock` separate from the root client workspace. Commit the server lockfile when the first member introduces a dependency graph. Keep server implementation modules out of root `crates/` and client dependency graphs. Protocol ownership remains in `protos/` under Apache-2.0.

## Incremental adoption

The workspace starts with no members and `servers/crates/.gitkeep` reserves the shared module directory. Cargo build, test and metadata commands require at least one member; no placeholder runtime or library is required. Existing Go services, the root Go module, protocol bindings and release images remain active until their individual ports are implemented.

Each port must update its project index and domain contracts, add its explicit workspace member and applicable validation and packaging integration. Run server Cargo commands from the root with `--manifest-path servers/Cargo.toml`. Root Cargo commands continue to own client packages. No server runtime is ported by this foundation change.

## Development execution

Root `pnpm dev` uses process-compose to manage each API and the frontend task group. It retains the existing workspace commands, Go Air watchers, Infisical injection, fixed loopback listeners and explicit migrations. See the [workflow contract](repository-workflow-contract.md) and [environment contract](repository-environment-contract.md). A later port changes the owning service launcher without moving shared server code across the license boundary.
