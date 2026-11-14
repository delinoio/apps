# apps-delino-docs foundation contract

## Scope

This contract owns the `delino-docs` documentation site at `apps/delino-docs/`:

- `apps/delino-docs/rspress.config.ts`: site configuration.
- `apps/delino-docs/docs/`: page content, navbar data (`docs/_nav.json`), per-directory sidebar order (`_meta.json`) and public static assets (`docs/public/`).
- `apps/delino-docs/theme/`: theme entry and brand style overrides.

## Runtime and Language

- Toolchain: Rspress `@rspress/core` 2.x on Rsbuild, React 19.
- Language: TypeScript for configuration and theme code, Markdown/MDX for content.
- Runtime: Node.js from root `.nvmrc` for builds. Production output is static HTML, CSS and JavaScript with no server runtime.
- Deployment target: Cloudflare Pages, production domain `docs.delino.io`.

## Users and Operators

- Delino users who need setup guides and references.
- Maintainers who author public documentation pages and deploy the site.

## Interfaces and Contracts

- Production origin: `https://docs.delino.io`. Rspress `base` is `/`.
- Content language: English (`lang: "en"`).

| Route ID                | Path                     | Purpose                     |
| ----------------------- | ------------------------ | --------------------------- |
| `home`                  | `/`                      | Documentation landing page. |
| `guide-introduction`    | `/guide/introduction`    | Product introduction.       |
| `guide-getting-started` | `/guide/getting-started` | First steps for new users.  |

Sidebar order for `/guide/` is owned by `docs/guide/_meta.json`. The navbar entries (`Guide` and `delino.io`) are owned by `docs/_nav.json`. `rspress.config.ts` must not set `themeConfig.nav` or `themeConfig.sidebar`, because either one disables the automatic navbar and sidebar generation from `_nav.json` and `_meta.json`. Use a directory-level `_meta.json` for any future section that needs an explicit sidebar.

- Build artifacts: `index.html` per route, `404.html`, `sitemap.xml` with absolute `https://docs.delino.io` URLs, `llms.txt` and `llms-full.txt`.
- `docs/public/_headers` sets Cloudflare Pages response headers: `X-Content-Type-Options: nosniff`, `Referrer-Policy: strict-origin-when-cross-origin` and `X-Frame-Options: DENY`.
- Rspress built-in full-text search is enabled. The index is generated at build time and is served as static assets; no AI search backend is used yet. If AI search is added, it must default to Cloudflare AI Search per repository defaults.

## Storage

The site has no persistent storage, user uploads or server-side cache. All content is committed source. Static assets of at least 512 KiB require Git LFS according to the repository workflow contract.

## Security

- The site is public. Content must not include secrets, internal hostnames, repository-internal paths or operational internals.
- The build requires no secrets or environment variables.
- Cloudflare Pages deployment credentials are not stored in the repository.

## Logging

The site has no runtime server logs. Build diagnostics come from the Rspress CLI output in local runs and CI logs. Cloudflare Pages owns request analytics.

## Build and Test

Run from the repository root:

- `pnpm install --frozen-lockfile`
- `pnpm --filter delino-docs dev`: local development server.
- `pnpm exec turbo run build --filter=delino-docs`: production build into `apps/delino-docs/dist/`.
- `pnpm exec turbo run typecheck --filter=delino-docs`: TypeScript type check.
- `pnpm --filter delino-docs preview`: serve the production build locally.

Remove `apps/delino-docs/dist/` after validation. CI does not build or deploy this site yet; add the build check to the workflow and its planner when the site enters CI.

## Dependencies and Integrations

- `@rspress/core`, `react` and `react-dom`, pinned in `apps/delino-docs/package.json`.
- Cloudflare Pages: static hosting for `docs.delino.io`. Configure the Pages project with build command `pnpm exec turbo run build --filter=delino-docs` and output directory `apps/delino-docs/dist`.
- [`delino-site`](project-delino-site.md): linked by absolute URL `https://delino.io`.

## Change Triggers

- Update [project-delino-docs.md](project-delino-docs.md) when paths, the production origin or deployables change.
- Update [project-delino-site.md](project-delino-site.md) and its foundation contract when cross-site links change.
- Update [repository-workflow-contract.md](repository-workflow-contract.md) when this site enters CI or a deployment workflow.
- Update `apps/AGENTS.md` only under the [instruction-update policy](README.md#instruction-update-policy).

## References

- [project-delino-docs.md](project-delino-docs.md)
- [repository-defaults.md](repository-defaults.md)
- [repository-workflow-contract.md](repository-workflow-contract.md)
- [Rspress documentation](https://rspress.rs/)
