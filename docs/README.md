# Documentation Catalog

## Purpose

`docs/` is the source of truth for repository contracts. Each project has one project index and one or more domain contracts. Read [root instructions](../AGENTS.md) and the relevant domain instructions before implementation. This catalog owns the complete document list; root instructions link here instead of maintaining a second list.

## Repository ownership

See the [repository boundary contract](repository-boundary-contract.md) for client/server directory ownership, contribution requirements and development in one checkout.

- [Server Rust workspace](servers-rust-workspace-contract.md) defines server Cargo ownership, shared modules and incremental adoption.

## Repository Defaults

- Repository-wide default technology choices and workflow defaults are defined in [repository defaults](repository-defaults.md).
- Repository licensing and third-party notice boundaries are defined in [license contract](repository-license-contract.md).
- Dependency security updates, audit evidence, and unresolved upstream constraints are recorded in [the dependency security contract](repository-dependency-security-contract.md).
- Repository configuration, environment ownership, and secret classification are defined in [environment contract](repository-environment-contract.md).
- Project and domain contracts must document deviations from those defaults when a different language, ID format, search backend, build toolchain, static-site deployment platform, or file storage/access pattern is chosen.

## Instruction Update Policy

Instruction files contain development procedures, directory ownership and contract navigation only. Ordinary feature additions, bug fixes, tests and behavior-contract changes do not by themselves require an `AGENTS.md` update. Update those files only when development procedures, directory ownership or repository/domain development rules change. Feature requirements belong in the owning contract and must not be duplicated in instruction files. Do not add issue-specific feature paragraphs to `AGENTS.md`.

Record new implementation status, validation results and unresolved limits in pull requests, issues and CI logs/artifacts. Include the source revision, commands, results and unperformed checks. Distinguish fixtures, builds and packaging from actual native/account/platform acceptance. Exclude secrets, user state and raw native content. Preserve existing validation records.

## Documentation Editing Rules

- Do not add issue or pull-request links or identifying numbers to repository instruction files or documentation. Describe requirements directly and link to the owning repository-local contracts. Domain, service and technical-documentation links remain allowed; record issue/PR traceability in pull requests, issues or CI records instead.
- These rules apply to documentation authoring and editing work, not general conversational summaries.
- Do not arbitrarily omit, delete, or simplify requested or source-backed content during documentation edits unless the user explicitly asks for that outcome.
- If documentation content, scope, or intent is ambiguous, ask the user before deciding what to remove, merge, or reinterpret.
- Update or create the relevant `AGENTS.md` in the same change only when development procedures, directory ownership or repository/domain development rules change; follow the [instruction-update policy](#instruction-update-policy).
- `docs/` remains the internal source of truth for contracts, architecture notes, repo-local paths, and implementation details. Any public documentation pages must curate from these contracts without documenting repository-internal implementation details unless the detail is a stable public interface, user-visible behavior, or explicitly public maintainer workflow.

## Naming Rules

- Project index docs: `docs/project-<project-id>.md`
- Domain contract docs: `docs/<domain>-<project-or-component>-<contract>.md`
- Domain prefix must be one of: `apps`, `cmds`, `servers`, `crates`, `protos`, `packages`
- Repository-level contract docs: `docs/repository-<topic>-contract.md`
- Use lowercase kebab-case identifiers and stable enum-style IDs in contract sections.

## Templates

- [Project index template](project-template.md): template for project index docs
- [Domain contract template](domain-template.md): template for domain contract docs

## Project catalog

### delcore

- [Project index](project-delcore.md)
- [Delcore server](servers-delcore-api-contract.md)
- [Protocol](protos-delcore-v1-contract.md)

### delidev

- [Project index](project-delidev.md)
- [Desktop shell](apps-delidev-desktop-contract.md)

### delitools

- [Project index](project-delitools.md)
- [Application and mobile distribution](apps-delitools-foundation.md)
- [API server](servers-delitools-api-contract.md)
- [Connect protocol](protos-delitools-v1-contract.md)

### devbase

- [Project index](project-devbase.md)
- [Web application](apps-devbase-web-contract.md)
- [API server](servers-devbase-api-contract.md)
- [Connect protocol](protos-devbase-v1-contract.md)

### delino-site

- [Project index](project-delino-site.md)
- [Web application](apps-delino-site-web-contract.md)
- [Shared Delino localization](apps-delino-site-localization-contract.md)

### delino-docs

- [Project index](project-delino-docs.md)
- [Public documentation app](apps-delino-docs-web-contract.md)
- [Shared Delino localization](apps-delino-site-localization-contract.md)

## Repository workflow

- [Workflow contract](repository-workflow-contract.md): affected CI, release gates, formatting, cache and validation evidence.
- [Environment contract](repository-environment-contract.md): committed configuration and service/release secret ownership.

Historical DeliDev validation describes the retired implementation and does not establish current shell acceptance. Preserve retained evidence in its original validation context.

### git-tracker

- [Project index](project-git-tracker.md)
- [Web application](apps-git-tracker-web-contract.md)
- [GitHub Action](packages-git-tracker-action-contract.md)
- [Connect protocol](protos-git-tracker-v1-contract.md)
- [Server](servers-git-tracker-api-contract.md)
- [Threat model](servers-git-tracker-threat-model.md)

### ui

- [Project index](project-ui.md)
- [Shared UI foundation](packages-ui-foundation.md)

### lynx-ui

- [Project index](project-lynx-ui.md)
- [Lynx UI foundation](packages-lynx-ui-contract.md)

### perfine

- [Project index](project-perfine.md)
- [Engine contract](crates-perfine-core-contract.md)
- [Adapters contract](crates-perfine-adapters-contract.md)
- [Cli contract](cmds-perfine-cli-contract.md)
- [Api contract](servers-perfine-api-contract.md)
- [Protocol contract](protos-perfine-v1-contract.md)
- [Web contract](apps-perfine-web-contract.md)
- [Distribution contract](packages-perfine-distribution-contract.md)

### lane

- [Project index](project-lane.md)
- [Application foundation](apps-lane-foundation.md)
