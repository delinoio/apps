# Repository boundary contract

## Ownership

This repository owns server runtimes, background workers, migrations, server configuration examples and OCI packaging under `servers/`, alongside web, desktop and mobile clients, protocols and consumer examples. Root `docs/` owns all project indexes and domain contracts. Server files use FSL-1.1-ALv2; other repository-owned files use Apache-2.0. Preserve third-party licenses and notices under the [license contract](repository-license-contract.md).

The Delcore protocol is `protos/delcore/v1`, with Go import path `github.com/delinoio/apps/protos/delcore/v1` and Connect bindings in `delcorev1connect`. Git Tracker uses `protos/gittracker/v1`, DevBase uses `protos/devbase/v1` and DeliTools uses `protos/delitools/v1`. Go consumers and protocol tests consume these packages directly through the root `github.com/delinoio/apps` Go module. Rust servers consume the local FSL-1.1-ALv2 `servers/crates/delino-protos` package, whose build script generates bindings from these same schemas. Keep canonical schemas and checked-in Go/TypeScript bindings in `protos/`. Generate Rust bindings only into Cargo output; do not check generated Rust source into `servers/`.

## Contribution boundary

Client, protocol and server contributions use one checkout, branch and review. No separate private repository access, companion checkout, `DELINO_APPS_DIR`, `client:setup`, protocol dependency pin or local `go.work` is required. Do not commit absolute paths or local `replace` directives.

Read root and affected scoped instructions. Keep public client behavior and protocol requirements in their owning contracts and server implementation details in root server contracts. Public product documentation describes supported behavior and interfaces. Review and CI records must exclude credentials, user state and raw account data. Live provider credentials are not prerequisites for fixture-based contributions.

## Changes across domains

Update affected project, protocol, client and server contracts in the same change. Preserve protocol field numbers, enum values and wire behavior. Run protocol lint, generation, freshness and FILE breaking checks against the comparison revision. Run Go vet, tests and consumer builds from the root, plus applicable frontend tests from the owning app.

Server changes require the owning server Cargo checks from the [server workspace contract](servers-rust-workspace-contract.md). Run the owning PostgreSQL integration tests against disposable databases and mocked providers, and build affected Rust server images for packaging validation. OCI builds use the root checkout and local protocol source. CI selects affected services and protocol consumers before runner allocation.

Record the source revision, commands, results and unperformed platform/account checks in review or CI records. Builds and fixtures do not establish live provider, deployment or native acceptance. No credentials or account state move through Git.

## Server Rust workspace

`servers/Cargo.toml` owns an independent Cargo workspace for server ports. Shared server Rust modules belong in `servers/crates/` under FSL-1.1-ALv2, separate from the Apache-2.0 client workspace and root `crates/`. See the [server Rust workspace contract](servers-rust-workspace-contract.md).
