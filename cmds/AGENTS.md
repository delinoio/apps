# cmds working instructions

- Follow root instructions and the owning contracts in `docs/`.
- `cmds/perfine` owns the Perfine CLI and MCP entry point. Read the [project index](../docs/project-perfine.md) and [CLI contract](../docs/cmds-perfine-cli-contract.md).
- Add Rust commands as explicit root workspace members when runtime implementation begins.

- `cmds/lane` owns the Go Lane CLI and local MCP in the root Go module. Read the [Lane project](../docs/project-lane.md), [CLI contract](../docs/cmds-lane-cli-contract.md) and [shared local-store contract](../docs/apps-lane-local-store-contract.md) before implementation.
