# Repository boundary contract

## Ownership

This repository owns FSL server runtimes, background workers, migrations, server configuration examples and OCI packaging under `servers/`, independent Apache-2.0 services under `oss/servers/`, and web, desktop and mobile clients, protocols and consumer examples. Root `docs/` owns all project indexes and domain contracts. Repository-owned files under the root `servers/` directory use FSL-1.1-ALv2; other repository-owned files, including `oss/servers/`, use Apache-2.0. Preserve third-party licenses and notices under the [license contract](repository-license-contract.md).

The Delcore protocol is `protos/delcore/v1`, with Go import path `github.com/delinoio/apps/protos/delcore/v1` and Connect bindings in `delcorev1connect`. Git Tracker uses `protos/gittracker/v1`, DevBase uses `protos/devbase/v1` and DeliTools uses `protos/delitools/v1`. Go consumers and protocol tests consume these packages directly through the root `github.com/delinoio/apps` Go module. FSL Rust servers consume the local FSL-1.1-ALv2 `servers/crates/delino-protos` package, whose build script generates bindings from these same schemas. Keep canonical schemas and checked-in Go/TypeScript bindings in `protos/`. Generate Rust bindings only into Cargo output; do not check generated Rust source into `servers/`.

## Deployment boundary

Private operator automation owns server rollout, infrastructure, monitoring assets and deployment evidence. This repository retains server runtime source, explicit migrations, OCI packaging, safe configuration examples, image publication and frontend/distribution publication. The release dispatch interface and authentication requirements are defined in the [workflow contract](repository-workflow-contract.md#project-release-preparation) and [environment contract](repository-environment-contract.md). Do not identify private deployment repositories in public source, documentation or review records.

## Contribution boundary

Client, protocol and server contributions use one checkout, branch and review. No separate private repository access, companion checkout, `DELINO_APPS_DIR`, `client:setup`, protocol dependency pin or local `go.work` is required. Do not commit absolute paths or local `replace` directives.

Read root and affected scoped instructions. Keep public client behavior and protocol requirements in their owning contracts and server implementation details in root server contracts. Public product documentation describes supported behavior and interfaces. Review and CI records must exclude credentials, user state and raw account data. Live provider credentials are not prerequisites for fixture-based contributions.

## Changes across domains

Update affected project, protocol, client and server contracts in the same change. Preserve protocol field numbers, enum values and wire behavior. Run protocol lint, generation, freshness and FILE breaking checks against the comparison revision. Run Go vet, tests and consumer builds from the root, plus applicable frontend tests from the owning app.

Server changes require the owning server Cargo checks from the [server workspace contract](servers-rust-workspace-contract.md). Run the owning PostgreSQL integration tests against disposable databases and mocked providers, and build affected Rust server images for packaging validation. OCI builds use the root checkout and local protocol source. CI selects affected services and protocol consumers before runner allocation.

Record the source revision, commands, results and unperformed platform/account checks in review or CI records. Builds and fixtures do not establish live provider, deployment or native acceptance. No credentials or account state move through Git.

## Server Rust workspace

`servers/Cargo.toml` owns an independent Cargo workspace for server ports. Shared server Rust modules belong in `servers/crates/` under FSL-1.1-ALv2, separate from the Apache-2.0 client workspace and root `crates/`. See the [server Rust workspace contract](servers-rust-workspace-contract.md).

## Independent Apache services

`oss/servers/<service-name>/` owns each independent Apache-2.0 service, including its runtime, migrations and packaging. This namespace is separate from the FSL server workspace at `servers/Cargo.toml`. Existing Apache clients, packages and protocols retain their canonical paths.

Before adding a service runtime, reserve its project ID and create its root `docs/project-<id>.md` index and owning `docs/servers-<id>-contract.md` contract. The project index records canonical paths and integration boundaries. The domain contract records build, test, deployment and distribution ownership. Follow `oss/AGENTS.md` for development rules.

Apache services must not depend on or copy FSL implementation code from `servers/`, including `servers/crates/`. Shared Apache protocol schemas remain in `protos/`; a service that needs Rust bindings generates them from those schemas into its own Cargo output rather than using the FSL `servers/crates/delino-protos` package or checking in duplicate generated source.

The namespace skeleton does not introduce a Cargo or npm workspace, development launcher or deployment target. Add explicit workspace membership, affected CI selection and service-owned build and deployment configuration with each runtime. Keep Apache Rust services outside the FSL server workspace; document any shared Apache workspace in the owning contracts when it is introduced.
