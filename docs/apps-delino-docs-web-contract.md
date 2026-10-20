# Delino Docs Web Application

## Scope

`apps/delino-docs` owns the public documentation app for multiple services at `https://docs.delino.io`. Public page sources live in `apps/delino-docs/docs`.

## Runtime and Language

Node.js 24 runs TypeScript and Rspress 2 tooling. Rspress provides static Markdown/MDX rendering and its default React theme. Cloudflare Pages is the static hosting target.

## Users and Operators

Visitors browse documentation for Delino services. Maintainers curate public pages and build the site.

## Interfaces and Contracts

- `/` introduces Delino Docs.
- `/guide/getting-started` explains how to browse the documentation.
- `/services/` is the service documentation directory. The scaffold has no service-specific guides.
- Future service pages use `/services/<service-id>/` with stable lowercase kebab-case service IDs and separate navigation sections.

English is the default language and retains these unprefixed routes. Korean pages use `/ko/`, `/ko/guide/getting-started` and `/ko/services/`. Both languages provide the same public content, translated navigation and a language selector that retains the current page. Do not redirect visitors automatically based on browser language. The Delino navigation link retains the selected language when opening the marketing site.

Rspress reads English sources from `apps/delino-docs/docs/en` and Korean sources from `apps/delino-docs/docs/ko`. Use matching relative page paths for each language.

Use Rspress navigation and sidebars to keep service documentation separate. The default theme provides responsive navigation and local full-text search; this requires no remote search backend or service credentials.

Run package-local `pnpm dev` on `127.0.0.1:46315`. A port conflict must stop startup. The production origin is `https://docs.delino.io` with base path `/`.

## Storage

The app serves static content and has no application database or account state. The default theme may store visitor theme preferences locally. `dist` and `.rspress` are ignored generated output.

## Security

Public pages must not expose secrets, internal contracts, private server details or user data. Curate documentation explicitly; do not copy or expose repository-root `docs/` automatically. No deployment credentials belong in the browser bundle.

## Logging

Rspress reports development, rendering and build failures through its CLI. Future service integrations must log structured failure events without credentials or raw user content.

## Build and Test

Run package-local `pnpm test` for TypeScript validation and a production build. `pnpm build` generates `dist` and includes the repository `LICENSE` and `NOTICE`. Remove generated `dist` after validation.

Affected CI runs `delino-docs-frontend` on Ubuntu and retains validation evidence. Changes in this app or its owning contracts select that job.

For Cloudflare Pages, build from the repository root with `pnpm --filter delino-docs build` and publish `apps/delino-docs/dist`. Production publication uses `.github/workflows/release-delino.yml` with the shared `delino@vMAJOR.MINOR.PATCH` tag. Release preparation updates both Delino package versions, and both apps deploy from the same source revision. The `delino-docs-production` environment supplies Cloudflare configuration. Dispatch the existing immutable tag from main to retry. Follow the [release procedure](repository-workflow-contract.md#web-publication). Cloudflare project creation, custom-domain binding and DNS require separate platform configuration. A local build does not establish platform or domain acceptance.

## Dependencies and Integrations

Pin the Rspress and TypeScript dependencies in the workspace lockfile. Rspress owns its compatible Rsbuild integration. The app does not call service APIs.

## Change Triggers

Keep project ownership, public route boundaries, workspace integration and affected CI aligned when this structure changes. Update instructions only for development procedures, ownership or repository/domain rules.

## References

- [Project index](project-delino-docs.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [License contract](repository-license-contract.md)
- [Rspress configuration](https://rspress.rs/api/config/config-basic)
