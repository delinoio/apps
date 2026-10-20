# Git Tracker

## Goal

Track numeric measurements for Git commits and identify regressions without repository snapshots. The service is free and supports public/private GitHub repositories and fork pull requests.

## Project ID

`git-tracker`

## Domain Ownership Map

- `apps/git-tracker`: Web application.
- `actions/git-tracker`: Official GitHub Action.
- `protos/gittracker/v1`: Connect schema and generated Go bindings.

Apps owns clients, the Action and protocol. Servers owns the Go runtime, migrations and persistence.

## Domain Contract Documents

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
