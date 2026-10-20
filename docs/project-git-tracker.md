# Git Tracker

## Goal

Track numeric measurements for Git commits and identify regressions without repository snapshots. The service is free and supports public/private GitHub repositories and fork pull requests.

## Project ID

`git-tracker`

## Domain Ownership Map

- `servers/git-tracker-api`: Go API, PostgreSQL persistence and background maintenance.
- `apps/git-tracker`: Web application.
- `actions/git-tracker`: Official GitHub Action.
- `protos/package.json`: Internal `@delino/protos` workspace for generated TypeScript exports and runtime dependencies.
- `protos/gittracker/v1`: Connect schema and generated Go and TypeScript bindings.

Client and protocol directories own their interfaces; `servers/git-tracker-api` owns the Go runtime, migrations and persistence in this repository.

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

Repository connection IDs are UUID v7 values. `github_repository_id` retains GitHub’s numeric external identifier. Reconnecting within a space preserves its connection ID and history. A disconnected repository can be connected to a different authorized space with a distinct connection ID; retained history does not cross spaces. History responses identify globally representative completed executions so date filtering cannot promote an older run. Deletion tombstones prevent delayed retries from recreating deleted executions or datasets within the same execution.
