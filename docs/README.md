# Documentation Catalog

`docs/` is the source of truth for repository contracts.

## Repository contracts

- [Technology defaults](repository-defaults.md)
- [Development and CI](repository-workflow-contract.md)
- [Configuration and secrets](repository-environment-contract.md)
- [Licensing](repository-license-contract.md)
- [Dependency security](repository-dependency-security-contract.md)
- [Issue tracking](repository-issue-tracker-contract.md)

## Templates

- [Project index](project-template.md)
- [Domain contract](domain-template.md)

## Project catalog

### delino-site

- [Project index](project-delino-site.md)
- [Marketing site foundation](apps-delino-site-foundation.md)

### delino-docs

- [Project index](project-delino-docs.md)
- [Documentation site foundation](apps-delino-docs-foundation.md)

Before implementing a new project, add `project-<id>.md`, at least one `<domain>-<project-or-component>-<contract>.md`, and their links here. Use lowercase kebab-case IDs and domain prefixes `apps`, `cmds`, `servers`, `crates`, `protos`, or `packages`.

## Instruction update policy

Keep feature behavior in its owning contract. Update `AGENTS.md` only when development procedures, directory ownership or development rules change. Keep validation evidence and issue or pull request traceability in pull requests, Linear issues and CI logs, rather than instruction files or documentation. See [root instructions](../AGENTS.md).
