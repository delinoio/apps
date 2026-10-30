# Labs documentation foundation

## Scope

`apps/labs-docs` owns the independent static Delino Labs documentation site.

## Runtime and Language

Use Node.js 24, the repository-pinned pnpm, Rspress 2.0.23, TypeScript configuration and Markdown pages. Use the stock Rspress theme without custom React components or visual assets.

## Users and Operators

Readers browse Labs documentation. Maintainers edit public pages and operate a dedicated Cloudflare Pages project.

## Interfaces and Contracts

- Canonical origin: `https://docs.labs.delino.io`; base: `/`; clean URLs enabled.
- `/` is the Overview page with the title `Delino Labs` and a link to Getting Started.
- `/getting-started` is the Getting Started page with navigation guidance. Do not invent product availability or workflows before product requirements exist.
- The Guide sidebar lists Overview and Getting Started. A TaiRouter group lists the eleven pages in `scripts/tairouter-pages.json` in onboarding order: overview, installation, configuration, organizations, providers/routing, clients, native compatibility, compatibility, operations, recovery and migration. The overview in every language links to the product and English guides.
- TaiRouter means Team AI Router and is an open-source Delino Labs product. Its public guides live in `docs/en/tairouter`, use the English `/tairouter/` and `/tairouter/guide/*` routes and are explicitly source-only in the localization registry for initial publication. Each language sidebar labels this group as English and links to the same English routes. Do not create translated copies or missing-language route aliases.
- Use the default search, theme toggle, page table of contents and previous/next links. TaiRouter pages are searchable in the English index; localized readers can follow the labelled English sidebar group. The stock Rspress 2.0.23 menu guesses nonexistent translated paths. A narrow build adapter in `scripts/english-guides-loader.cjs` keeps the default theme and sends language selection on an English-only page to the selected language overview, in both server rendering and client navigation. Its exact upstream hook signature is checked during compilation. Remove the adapter when the stock theme handles missing translations or the guides receive translations.
- The default responsive theme collapses navigation on mobile. Preserve keyboard navigation, visible focus and Escape dismissal for dialogs. Search handles empty results; unknown routes use the default 404 page. Static pages require no account, permission or loading flow.
- Root `pnpm dev:labs-docs` and package-local `pnpm dev` bind to `127.0.0.1:46330`. Development uses Infisical and Portless; the preview wrapper forwards termination signals. Both listeners use strict loopback ports. Preview uses fixed loopback port `46331`.
- Cloudflare Pages target settings: project `labs-docs`, repository `delinoio/apps`, production branch `main`, repository-root build command `pnpm --filter @apps/labs-docs build`, output `apps/labs-docs/doc_build`, `NODE_VERSION=24`, `PNPM_VERSION=12.14.0`, custom domain `docs.labs.delino.io`.
- These are deployment settings for operators, not evidence of provisioned hosting or DNS. CI validates only and must not deploy.

## Localization

Follow the [shared Labs localization contract](apps-labs-localization-contract.md) for catalogs, localized routes, ChatGPT plan generation, credential ownership and offline validation.

## Storage

English source pages are versioned under `apps/labs-docs/docs/en`; translated pages use sibling language directories. Generated `doc_build` output and Rspress cache directories are ignored. No runtime storage is required.

## Security

Publish user-facing content only. Exclude credentials, private configuration and repository-internal implementation details. No secrets or external services are required to build or browse the site.

## Logging

Retain Rspress build diagnostics. Route validation reports structured route and result events without credentials or reader data.

## Build and Test

- Development: `pnpm dev:labs-docs` from the root, or `pnpm dev` from the app.
- Build: `pnpm --filter @apps/labs-docs build`.
- Preview: `pnpm --filter @apps/labs-docs preview`.
- Validation: `pnpm test` from `apps/labs-docs` builds the site and checks all registered language route artifacts and the eleven English TaiRouter routes, page titles, cross-page navigation, canonical links and the 404 artifact. Content tests check local TaiRouter links, navigation coverage, exact English-only declarations, public-content limits and removal of the former publisher.
- CI: `labs-docs-frontend` uses the repository affected planner. Its uncached `ci:checks` runs the complete app test. Source/config, toolchain and owning contracts select the job. Shared localization changes select all translation consumers.
- Manually inspect desktop/mobile navigation, search, theme toggle, focus and 404 behavior when changing the theme. Local validation does not establish Cloudflare account, DNS or production acceptance.

## Dependencies and Integrations

The app depends on `@rspress/core` and uses the app-local preview wrapper. It does not add entries to the OSS project selector or duplicate Labs pages in `apps/public-docs`.

## Change Triggers

Update [the project index](project-labs-docs.md) for ownership or publishing changes, this contract for route/runtime changes and [the catalog](README.md) for document inventory changes. Update `apps/AGENTS.md` when development rules or ownership change.

## References

- [Project index](project-labs-docs.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Repository workflow](repository-workflow-contract.md)

## Repository ownership

Source is maintained in `delinoio/apps`. The public origins and TaiRouter source links remain unchanged. Development is available at `https://labs-docs.localhost` through Portless on app port `46330`; preview uses `46331`. Build and test commands require no secrets. Cloudflare project/domain transfer and production acceptance remain operator work; this migration does not deploy or change account settings.
