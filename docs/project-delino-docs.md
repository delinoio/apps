# Project: delino-docs

## Goal

Provide the public Delino documentation site with user guides and references.

## Project ID

`delino-docs`

## Domain Ownership Map

- `apps/delino-docs/`: Rspress static documentation site served at `https://docs.delino.io`.

The project has one deployable. It has no `cmds`, `servers`, `crates`, `protos` or `packages` paths.

## Domain Contract Documents

- [apps-delino-docs-foundation.md](apps-delino-docs-foundation.md)

## Cross-Domain Invariants

- The canonical production origin is `https://docs.delino.io`. The Rspress `sitemap.siteUrl` value must match it.
- The site links to [`delino-site`](project-delino-site.md) at `https://delino.io` through absolute URLs only. It must not import source from `apps/delino-site/`.
- Build output is written to `apps/delino-docs/dist/`, which is ignored generated output.
- The site follows repository defaults: Rspress toolchain and Cloudflare Pages deployment. There are no deviations.

## Change Policy

- Update [apps-delino-docs-foundation.md](apps-delino-docs-foundation.md) and this index together when routes, the production origin, the toolchain or the deployment target change.
- Update [project-delino-site.md](project-delino-site.md) when the cross-site link contract changes.
- Update the [documentation catalog](README.md) when contracts are added, renamed or removed.
- Update `apps/AGENTS.md` only when development procedures, directory ownership or development rules change; follow the [instruction-update policy](README.md#instruction-update-policy).
- Record implementation status, validation results and unresolved limits in pull requests, Linear issues and CI logs/artifacts.

## References

- [repository-defaults.md](repository-defaults.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [project-delino-site.md](project-delino-site.md)
- [project-template.md](project-template.md)
