# Project: Delino Docs

## Goal

Provide one public documentation app for multiple Delino services at `https://docs.delino.io`.

## Project ID

`delino-docs`. The npm workspace is `delino-docs`.

## Domain Ownership Map

- `apps/delino-docs`: independently built public documentation app.
- `apps/delino-docs/docs`: curated public Markdown and MDX pages.

## Domain Contract Documents

- [Documentation app](apps-delino-docs-web-contract.md)

## Cross-Domain Invariants

The documentation app serves multiple services. Each service owns a section under `/services/<service-id>/`. Service IDs use stable lowercase kebab-case values. The scaffold does not select which services publish documentation.

Repository-root `docs/` remains the internal source of truth. Public pages describe user-facing behavior, supported workflows and stable public interfaces. Do not publish internal contracts automatically.

TypeScript and Rspress replace the default new-project Go language because this app generates a static documentation site. Cloudflare Pages is the hosting target. Service runtimes and the marketing website remain independently owned applications.

## Change Policy

Update this index and the web contract together when paths, interfaces or deployment boundaries change. Follow the [instruction-update policy](README.md#instruction-update-policy). Record validation and unresolved limits in commits, PRs or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delino marketing website](project-delino-site.md)
