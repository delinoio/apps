# packages working instructions

- Follow root instructions. This directory owns shared client packages.
- For `packages/ui`, read [the project index](../docs/project-ui.md) and [the foundation contract](../docs/packages-ui-foundation.md).
- Keep application behavior in its owning app and preserve imported component licenses.
- For `packages/lynx-ui`, read [the project index](../docs/project-lynx-ui.md) and [the foundation contract](../docs/packages-lynx-ui-contract.md). Keep host integration requirements explicit and distinguish bundle checks from native acceptance.
- Run `pnpm test` from the changed package and affected frontend directories.

- Perfine ownership: [project](../docs/project-perfine.md); [distribution contract](../docs/packages-perfine-distribution-contract.md).

- `packages/lane` owns the Lane npm installer and common agent skill/guides. Read the [Lane project](../docs/project-lane.md) and [distribution contract](../docs/packages-lane-distribution-contract.md); CLI source remains in `cmds/lane`.
