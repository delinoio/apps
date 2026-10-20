# protos working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `protos/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- Follow root `AGENTS.md` and the owning project/domain contracts.

- Use local contracts and checks. Update affected server contracts and run server integration validation in this checkout under the [repository boundary contract](../docs/repository-boundary-contract.md).
- Write schemas and comments in English.
- Keep generated Go code beside its owning schemas as defined in the protocol contract. Generation must preserve schema and instruction files.

## Owning contracts

- [project-delcore](../docs/project-delcore.md)
- [protos-delcore-v1-contract](../docs/protos-delcore-v1-contract.md)
- [servers-delcore-api-contract](../docs/servers-delcore-api-contract.md)
- [Git Tracker protocol](../docs/protos-git-tracker-v1-contract.md)
