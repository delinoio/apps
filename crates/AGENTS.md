# crates working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `crates/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Development and validation

- Follow root `AGENTS.md` and each crate-specific project document.

- Keep repository and domain rules in the appropriate `AGENTS.md` files.

- Write all source and comments in English.

- Prefer enums over free-form strings for stable internal and external contracts.

- Add new crates as explicit workspace members in root `Cargo.toml`.

- Keep crate naming aligned with project IDs when possible.

- Document behavior contracts in project index docs and relevant crate-domain docs before large implementation changes.

- Planned crate paths must not be added as workspace members until the crate skeleton exists.

- Prefer minimal default features and keep optional capabilities opt-in for size-sensitive crates.

- Keep proc-macro crates and runtime crates separated by explicit crate boundaries.

- Test cleanup must remove only the fixture-owned directory, never the process-wide temporary directory or sibling fixtures.

## Owning contracts

- [Repository defaults](../docs/repository-defaults.md)
- [Repository workflow](../docs/repository-workflow-contract.md)
- Register each new project and its domain contracts in [the documentation catalog](../docs/README.md) before implementation.
