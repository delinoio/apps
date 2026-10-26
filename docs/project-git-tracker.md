# Git Tracker

## Goal

Track numeric measurements for Git commits and identify regressions without repository snapshots. The service is free and supports public/private GitHub repositories and fork pull requests.

## Project ID

`git-tracker`

## Domain Ownership Map

- `servers/crates/delino-protos`: FSL-1.1-ALv2 Rust protocol crate that generates bindings from the owning schemas at Cargo build time.
- `servers/Cargo.toml` and `servers/crates/`: independent server Rust workspace and shared server modules for incremental ports; see the [server Rust workspace contract](servers-rust-workspace-contract.md).

- `servers/scripts/dev-api.mjs`: shared API development launcher.
- `protos/scripts/`: shared Buf entry resolution and protocol breaking checks.

- `servers/git-tracker-api`: Rust API, PostgreSQL persistence, background maintenance and OCI packaging.
- `apps/git-tracker`: Web application.
- `actions/git-tracker`: Official GitHub Action.
- `protos/package.json`: Internal `@repo/protos` workspace for generated TypeScript exports and runtime dependencies.
- `protos/gittracker/v1`: Connect schema and generated Go and TypeScript bindings.

Stored executions use the current protobuf JSON representation without a retired Go storage adapter. Client and protocol directories own their interfaces; `servers/git-tracker-api` owns the Rust runtime, migrations and persistence in this repository.

The API directory contains Rust source and tests plus canonical SQL migrations. The retired Go implementation and Air configuration are removed; generated Go protocol bindings and Go consumer examples remain supported.

## Domain Contract Documents

- [Server](servers-git-tracker-api-contract.md)
- [Threat model](servers-git-tracker-threat-model.md)
- [Web application](apps-git-tracker-web-contract.md)
- [GitHub Action](packages-git-tracker-action-contract.md)
- [Connect protocol](protos-git-tracker-v1-contract.md)

## Cross-Domain Invariants

The service URL is https://git-tracker.delino.io and the API URL is https://git-tracker.api.delino.io. Logto owns identity and Delcore owns spaces, memberships and service permissions. Use UUID v7 for service entities; GitHub references and user-defined dataset, row and column IDs retain their external shapes. JSON numbers cross protobuf boundaries as decimal strings without binary floating-point conversion. Completion describes measurement completion only.

## Change Policy

Update the owning domain contracts and this index together for boundary changes. Instruction changes cover ownership and development rules only. Preserve validation evidence in review and CI records.

## References

- [Defaults](repository-defaults.md)
- [Boundary](repository-boundary-contract.md)

The Go calling example is `examples/git-tracker-client`. Public setup and calling guidance is curated under `apps/delino-docs/docs/services/git-tracker`.

Repository connection IDs are UUID v7 values. `github_repository_id` retains GitHub’s numeric external identifier. Reconnecting within a space preserves its connection ID and history. A disconnected repository can be connected to a different authorized space with a distinct connection ID; retained history does not cross spaces. History responses identify globally representative completed branch executions so date filtering cannot promote an older run and pull-request executions never become representatives. Deletion tombstones prevent delayed retries from recreating deleted executions or datasets within the same execution.
