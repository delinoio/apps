# Delino Web Application

## Scope

`apps/delino-site` owns the web application at `https://delino.io`. Provide the Delino marketing website.

## Runtime and Language

TypeScript, React 19 and Rsbuild follow the repository frontend stack. Node.js 24 runs development and build tooling. Cloudflare Pages serves the static build.

## Users and Operators

Visitors learn about Delino through the marketing website. Maintainers build and publish the web application.

## Interfaces and Contracts

The root route `/` has the title `Delino` and retains the empty React root. The product route `/products/fixkey` renders a Korean-language FixKey landing page with a product-specific title. Accept an optional trailing slash. Other routes retain the empty scaffold.

The FixKey landing page introduces Korean Dubeolsik and US English QWERTY correction, local input processing, correction controls and setup requirements. It includes a clearly labeled browser demonstration with fixed examples, feature and privacy sections, native expandable FAQ items, and platform-specific download selection. The demonstration does not run the native correction engine or capture keyboard input.

Download links target the published FixKey `0.1.7` macOS ARM64 DMG, Windows x64 EXE and Linux X11 x64 AppImage. Link release notes to the matching FixKey release rather than the repository-wide latest release. Selecting a platform updates its installer link and setup note. State the Windows unsigned-publisher warning and the Linux X11 requirement. Do not imply verified application compatibility from target-platform availability. Keep product claims aligned with the [FixKey project](project-fixkey.md) and [desktop contract](apps-fixkey-desktop-contract.md).

Navigation links target on-page sections. Provide semantic landmarks, keyboard-visible focus, native FAQ keyboard interaction, responsive desktop and mobile layouts, and reduced-motion support. Load Noto Sans KR through Google Fonts with system fallbacks; this external font request contains no input or correction content. Canonical and social metadata identify `https://delino.io/products/fixkey` only on the product route.

This project owns the marketing website. DevBase and DeliTools remain separate applications with their own domains and contracts.

Run package-local `pnpm dev` on `127.0.0.1:46314`. A port conflict must stop startup. The production domain does not replace the loopback development address.

## Storage

The scaffold has no persistent data, cookies or account state. `dist` is ignored generated build output.

## Security

The browser bundle must contain no service credentials. Deployment credentials remain outside the repository. The marketing scaffold has no authenticated or payment operations.

## Logging

Rsbuild reports startup and build failures. Future service integrations must emit structured failure events without credentials or raw user content.

## Build and Test

Run `pnpm test` in this directory for TypeScript checks and a production build. `pnpm build` emits `dist`. Affected CI runs these checks on Ubuntu. Remove generated `dist` after validation.

The build emits a dedicated `products/fixkey/index.html` with product metadata and root-absolute asset URLs so nested-route requests load correctly on static hosting.

Cloudflare Pages builds the `delino-site` workspace from the repository root with `pnpm --filter delino-site build` and publishes `apps/delino-site/dist`. Include repository `LICENSE` and `NOTICE` in distributions. Domain binding and deployment require separate platform configuration; a local build does not establish deployment acceptance.

Production publication uses `.github/workflows/release-delino-site.yml` after `release-project.yml` commits and tags the selected version. The owning `delino-site-production` environment supplies Cloudflare configuration. Existing immutable tags can be dispatched again from main for deployment recovery. Follow the [release procedure](repository-workflow-contract.md#web-publication); local builds do not establish Cloudflare or domain acceptance.

## Dependencies and Integrations

Use the repository-pinned React, Rsbuild and TypeScript versions. The marketing site is independent of application runtime and billing services.

## Change Triggers

Keep the project index, workspace registration and workflow contract aligned when paths or validation procedures change. Update instructions only for development procedures, ownership or repository/domain rules.

## References

- [Project index](project-delino-site.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Delcore](project-delcore.md)
