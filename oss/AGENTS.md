# Independent Apache services

## Ownership and contracts

- `oss/servers/<service-name>/` owns independent Apache-2.0 service runtimes, migrations and packaging. Follow the [repository boundary contract](../docs/repository-boundary-contract.md#independent-apache-services) and [license contract](../docs/repository-license-contract.md#independent-apache-services).
- Keep project indexes and domain contracts in root `docs/`. Read the owning project index and server contract before changing a service.
- Before adding a runtime, reserve a project ID and create `docs/project-<id>.md` and at least `docs/servers-<id>-contract.md` under the root onboarding procedure. Add service-scoped instructions when its development rules require them.

## License and dependency boundary

- License repository-owned files and distributions in this subtree under Apache-2.0. Include `oss/LICENSE` and `oss/NOTICE` with distributions, together with required third-party notices.
- Do not depend on or copy FSL implementation code from the root `servers/` directory, including `servers/crates/`. Preserve imported code licenses and notices.
- Keep shared protocol schemas in root `protos/`. Generate Rust bindings into Cargo output from those schemas; do not use the FSL server protocol crate or duplicate checked-in generated source.

## Build and validation ownership

- Keep Apache services outside the FSL Cargo workspace at `servers/Cargo.toml`. Document build, test and deployment ownership in each service contract.
- Add explicit workspace membership and affected CI coverage with runtime implementation. Do not add placeholder manifests, shared workspaces or deployment targets for the namespace alone.
- Follow root technology, environment and development-command rules. Use disposable databases and mocked providers for integration checks, and record actual validation coverage in pull requests or CI records.
