# servers working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `servers/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- Follow root `AGENTS.md` and the owning project/domain contracts.
- Run service-local `pnpm dev` for one Go API or root `pnpm dev` for the full workspace. Follow the owning server contract for required configuration and database preparation; the pinned Air watcher does not provision dependencies or run migrations. Root development passes Compose database URLs to the API launchers after Infisical injection; preserve the environment precedence and secret boundaries in the environment contract. Keep each API's Infisical development settings in its owning folder and select that folder explicitly in its development command.
- All repository-owned files under `servers/` use FSL-1.1-ALv2; preserve third-party licenses and notices. Include `servers/LICENSE` and `servers/NOTICE` in server distributions.
- Run server checks from the repository root with `go vet ./servers/...`, `go build ./servers/delcore-api/cmd/... ./servers/devbase-api/cmd/... ./servers/git-tracker-api/cmd/...` and `go test -race ./servers/...`. Run the owning PostgreSQL and OCI checks when affected. Protocol source remains in root `protos/`; do not duplicate it under servers.

- Write Go code and comments in English; use `log/slog` structured logging and never log secrets or sensitive payloads.

## Owning contracts

- [project-delcore](../docs/project-delcore.md)
- [servers-delcore-api-contract](../docs/servers-delcore-api-contract.md)
- [protocol contract](../docs/protos-delcore-v1-contract.md)
- [repository boundary](../docs/repository-boundary-contract.md)
- [repository-environment-contract](../docs/repository-environment-contract.md)
- [DevBase project](../docs/project-devbase.md)
- [DevBase server](../docs/servers-devbase-api-contract.md)
- [DevBase protocol](../docs/protos-devbase-v1-contract.md)
- [Git Tracker project](../docs/project-git-tracker.md)
- [Git Tracker server](../docs/servers-git-tracker-api-contract.md)
- [Git Tracker threat model](../docs/servers-git-tracker-threat-model.md)
