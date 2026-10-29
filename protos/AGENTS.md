# protos working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `protos/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- `protos/scripts/` owns shared Buf entry resolution and breaking checks; follow the [repository workflow](../docs/repository-workflow-contract.md).

- Follow root `AGENTS.md` and the owning project/domain contracts.

- Use local contracts and checks. Update affected server contracts and run server integration validation in this checkout under the [repository boundary contract](../docs/repository-boundary-contract.md).
- Write schemas and comments in English.
- Keep checked-in generated protobuf and Connect bindings under `protos/`, beside their owning schemas as defined in the protocol contracts. The Rust package in `servers/crates/delino-protos` generates its bindings from those schemas into Cargo output during compilation; do not check those outputs in or copy them into server sources. Run root `pnpm proto:generate` for all languages; generation must preserve schema and instruction files.
- The private `@repo/protos` workspace owns TypeScript exports and their protobuf runtime dependency; applications consume these exports instead of generating app-local copies.

## Owning contracts

- [project-delcore](../docs/project-delcore.md)
- [protos-delcore-v1-contract](../docs/protos-delcore-v1-contract.md)
- [servers-delcore-api-contract](../docs/servers-delcore-api-contract.md)
- [DevBase protocol](../docs/protos-devbase-v1-contract.md)
- [DevBase server](../docs/servers-devbase-api-contract.md)
- [Git Tracker protocol](../docs/protos-git-tracker-v1-contract.md)
- [DeliTools protocol](../docs/protos-delitools-v1-contract.md)
- [DeliTools server](../docs/servers-delitools-api-contract.md)

- Perfine ownership: [project](../docs/project-perfine.md); [protocol contract](../docs/protos-perfine-v1-contract.md).

- Planned `protos/lane/v1` owns Lane schemas, generated bindings and cross-language conformance fixtures. Read the [Lane project](../docs/project-lane.md) and [Lane protocol contract](../docs/protos-lane-v1-contract.md).
