# servers working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `servers/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts below before changing behavior, including affected cross-domain consumers.

## Rust workspace ownership

- Default to Rust for new server implementations unless the owning project or domain contract specifies another language. Follow the [repository defaults](../docs/repository-defaults.md).
- `servers/Cargo.toml` owns the independent server Cargo workspace. Add explicit members only when implementing each port; keep server packages out of the root client workspace.
- Put shared server Rust modules in `servers/crates/` and inherit the server workspace license and publication settings. Follow the [server Rust workspace contract](../docs/servers-rust-workspace-contract.md).
- In root Turbo development, only the `//#server-build` task may invoke Cargo builds. Ported API launchers must execute the built binaries directly; do not start per-service Cargo build or watch processes. Follow the server Rust workspace contract for build dependencies and manual restarts.
- Run Cargo checks for implemented server members with `--manifest-path servers/Cargo.toml` from the repository root, in addition to applicable root checks.

## Development and validation

- `servers/scripts/` owns development helpers shared by the APIs; follow the [repository workflow](../docs/repository-workflow-contract.md).

- Follow root `AGENTS.md` and the owning project/domain contracts.
- Run service-local `pnpm dev` for one API or root `pnpm dev` for Turborepo to manage the full development session. Follow the owning server contract for required configuration and database preparation; the development launchers do not provision dependencies or run migrations. Root development passes Compose database URLs to the API launchers after Infisical injection; preserve the environment precedence and secret boundaries in the environment contract. Keep each API's Infisical development settings in its owning folder and select that folder explicitly in its development command.
- Delcore development startup requires a valid YAML catalog after explicit migrations. The shared builder does not restart for YAML-only edits; restart the API task to apply them. Follow the owning server contract for the file format and recovery procedure.
- All repository-owned files under `servers/` use FSL-1.1-ALv2; preserve third-party licenses and notices. Include `servers/LICENSE` and `servers/NOTICE` in server distributions.
- Run server checks from the repository root with `cargo fmt --manifest-path servers/Cargo.toml --all --check`, `cargo clippy --locked --manifest-path servers/Cargo.toml --workspace --all-targets -- -D warnings` and `cargo test --locked --manifest-path servers/Cargo.toml`. Run the owning PostgreSQL and OCI checks when affected. Protocol source remains in root `protos/`; do not duplicate it under servers.

- Write code and comments in English; use `log/slog` for Go and `tracing` for Rust. Never log secrets or sensitive payloads.

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
- [DeliTools project](../docs/project-delitools.md)
- [DeliTools server](../docs/servers-delitools-api-contract.md)
- [DeliTools protocol](../docs/protos-delitools-v1-contract.md)

- Delcore, DevBase, Git Tracker and DeliTools production binaries and OCI images are Rust. Their runtime source and tests belong to the server Cargo workspace; Go service implementations and Air configurations are removed. Build the server workspace before direct service-local development. Shared runtime modules belong in `crates/server-common`; protocol generation belongs in `servers/crates/delino-protos` under the server workspace license.
