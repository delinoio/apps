# Project: delino-site

## Goal

Provide the public Delino marketing site that introduces the product and routes readers to the documentation site.

## Project ID

`delino-site`

## Domain Ownership Map

- `apps/delino-site/`: Rspress static marketing site served at `https://delino.io`.

The project has one deployable. It has no `cmds`, `servers`, `crates`, `protos` or `packages` paths.

## Domain Contract Documents

- [apps-delino-site-foundation.md](apps-delino-site-foundation.md)

## Cross-Domain Invariants

- The canonical production origin is `https://delino.io`. The Rspress `sitemap.siteUrl` value must match it.
- The site links to [`delino-docs`](project-delino-docs.md) at `https://docs.delino.io` through absolute URLs only. It must not import source from `apps/delino-docs/`.
- Build output is written to `apps/delino-site/dist/`, which is ignored generated output.
- The site follows repository defaults: Rspress toolchain and Cloudflare Pages deployment. There are no deviations. It deploys through the `static-sites` GitHub environment and has no deployment dependencies, according to the [deployment contract](repository-deployment-contract.md).

## Change Policy

- Update [apps-delino-site-foundation.md](apps-delino-site-foundation.md) and this index together when routes, the production origin, the toolchain or the deployment target change.
- Update [project-delino-docs.md](project-delino-docs.md) when the cross-site link contract changes.
- Update the [documentation catalog](README.md) when contracts are added, renamed or removed.
- Update `apps/AGENTS.md` only when development procedures, directory ownership or development rules change; follow the [instruction-update policy](README.md#instruction-update-policy).
- Record implementation status, validation results and unresolved limits in pull requests, Linear issues and CI logs/artifacts.

## References

- [repository-defaults.md](repository-defaults.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [repository-deployment-contract.md](repository-deployment-contract.md)
- [project-delino-docs.md](project-delino-docs.md)
- [project-template.md](project-template.md)
