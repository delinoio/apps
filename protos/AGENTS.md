# protos working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `protos/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- Follow root `AGENTS.md` and the owning project/domain contracts.

- Use local contracts and checks. Update affected server contracts and run server integration validation in this checkout under the [repository boundary contract](../docs/repository-boundary-contract.md).
- Write schemas and comments in English.
- Keep all generated protobuf and Connect bindings under `protos/`, beside their owning schemas as defined in the protocol contracts. Run root `pnpm proto:generate` for all languages; generation must preserve schema and instruction files.
- The private `@delino/protos` workspace owns TypeScript exports and their protobuf runtime dependency; applications consume these exports instead of generating app-local copies.

## Owning contracts

- [project-delcore](../docs/project-delcore.md)
- [protos-delcore-v1-contract](../docs/protos-delcore-v1-contract.md)
- [servers-delcore-api-contract](../docs/servers-delcore-api-contract.md)
- [Git Tracker protocol](../docs/protos-git-tracker-v1-contract.md)
