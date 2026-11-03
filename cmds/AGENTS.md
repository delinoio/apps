# cmds working instructions

- Follow root instructions and the owning contracts in `docs/`.
- `cmds/perfine` owns the Perfine CLI and MCP entry point. Read the [project index](../docs/project-perfine.md) and [CLI contract](../docs/cmds-perfine-cli-contract.md).
- Add Rust commands as explicit root workspace members when runtime implementation begins.

- `cmds/lane` owns the Rust Lane CLI and local MCP in the root Cargo workspace. Read the [Lane project](../docs/project-lane.md), [CLI contract](../docs/cmds-lane-cli-contract.md) and [shared local-store contract](../docs/apps-lane-local-store-contract.md) before implementation.

- `cmds/anydash` owns the Rust AnyDash CLI and shared collector in the root Cargo workspace. Read the [AnyDash project](../docs/project-anydash.md), [CLI contract](../docs/cmds-anydash-cli-contract.md) and [provider adapters contract](../docs/cmds-anydash-provider-adapters-contract.md) before implementation.
