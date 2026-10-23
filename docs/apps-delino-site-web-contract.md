# Delino Web Application

## Scope

`apps/delino-site` owns the web application at `https://delino.io`. Provide the Delino marketing website.

## Runtime and Language

TypeScript, React 19 and Rsbuild follow the repository frontend stack. Node.js 24 runs development and build tooling. Cloudflare Pages serves the static build.

## Users and Operators

Visitors learn about Delino through the marketing website. Maintainers build and publish the web application.

## Interfaces and Contracts

The root route `/` has the title `Delino`. The homepage introduces Delino as a software company that helps developers focus on their work. Its content order is the company purpose, company introduction, products, development principles, contact and footer. Git Tracker is the primary developer tool. Its product summary links to documentation and source code. Product illustrations are static examples, not live dashboards or evidence of service availability. The page directs visitors to product documentation and releases for availability and compatibility.

Header links navigate to the introduction, products and development principles. The primary hero action navigates to products. Contact links open `mailto:support@delino.io`; the address remains visible. The footer retains documentation and source-code links. The homepage adds no account, payment or contact-submission service. API clients require subsequent implementation.

English uses `/`; every other registered language uses `/<language>/`, such as `/ko/` and `/ja/`. Each registered language has an independent static HTML entry with matching `lang` metadata and a localized homepage description. Language selection preserves the query and fragment. Documentation links retain the selected language. Company content uses the `delino.*` catalog namespace. Follow the [localization contract](apps-delino-site-localization-contract.md) for catalogs and automated generation.

The homepage uses a centered layout up to 1160px wide, with a two-column hero and introduction, a product grid and three development-principle columns. At widths of 700px or less, these sections stack in reading order and header navigation remains visible on a second row. Preserve usable layouts from 320px wide, visible keyboard focus, a skip link, labeled language selection, semantic headings and touch targets of at least 44px. Decorative diagrams remain hidden from assistive technology. If the locale catalog fails to load, render an English error message and a reload action independently of that catalog, and emit the structured locale failure event.

This project owns the marketing website. DevBase and DeliTools remain separate applications with their own domains and contracts.

Run package-local `pnpm dev` on `127.0.0.1:46314`. A port conflict must stop startup. The production domain does not replace the loopback development address.

## Storage

The scaffold has no persistent data, cookies or account state. `dist` is ignored generated build output.

## Security

The browser bundle must contain no service credentials. Deployment credentials remain outside the repository. The marketing scaffold has no authenticated or payment operations.

## Logging

Rsbuild reports startup and build failures. Future service integrations must emit structured failure events without credentials or raw user content.

## Build and Test

Run `pnpm test` in this directory for offline translation validation, TypeScript checks and a production build. `pnpm build` also validates translations before emitting `dist` and includes the repository `LICENSE` and `NOTICE` and the shared component `LICENSE.shadcn`. Affected CI runs these checks on Ubuntu without ChatGPT credentials. Remove generated `dist` after validation.

Cloudflare Pages builds the `delino-site` workspace from the repository root with `pnpm --filter delino-site build` and publishes `apps/delino-site/dist`. Include repository `LICENSE` and `NOTICE` in distributions. Domain binding and deployment require separate platform configuration; a local build does not establish deployment acceptance.

Production publication uses `.github/workflows/release-delino.yml` after `release-project.yml` commits both Delino app versions and creates the shared `delino@vMAJOR.MINOR.PATCH` tag. Both apps deploy from the same source revision. The shared `delino-site-production` environment supplies the deployment token and account ID. `CLOUDFLARE_PAGES_PROJECT_SITE` selects this app’s Pages project; `CLOUDFLARE_PAGES_PROJECT_DOCS` selects the documentation app’s project. Existing immutable tags can be dispatched again from main for deployment recovery. Follow the [release procedure](repository-workflow-contract.md#web-publication); local builds do not establish Cloudflare or domain acceptance.

## Dependencies and Integrations

`@delino/ui` supplies shared components, styles and PostCSS configuration. Shared labels and native selects render language selection. Shared buttons preserve call-to-action links, and shared cards preserve product articles. Follow [the shared UI contract](packages-ui-foundation.md) for stylesheet imports and component adoption.

Use the repository-pinned React, Rsbuild and TypeScript versions. Declare `@types/node` as a direct development dependency for the Node.js APIs used by `rsbuild.config.ts`; type checking must not depend on transitive type hoisting from pnpm's store layout. The marketing site is independent of application runtime and billing services.

## Change Triggers

Keep the project index, workspace registration and workflow contract aligned when paths or validation procedures change. Update instructions only for development procedures, ownership or repository/domain rules.

## References

- [Project index](project-delino-site.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Delcore](project-delcore.md)
