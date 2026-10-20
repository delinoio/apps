# Delino Web Application

## Scope

`apps/delino-site` owns the web application at `https://delino.io`. Provide the Delino marketing website.

## Runtime and Language

TypeScript, React 19 and Rsbuild follow the repository frontend stack. Node.js 24 runs development and build tooling. Cloudflare Pages serves the static build.

## Users and Operators

Visitors learn about Delino through the marketing website. Maintainers build and publish the web application.

## Interfaces and Contracts

The root route `/` has the title `Delino`. The header provides a documentation link and an English/Korean language selector. English is the default; `?lang=en` and `?lang=ko` select the language explicitly, and unsupported values fall back to English. The HTML language follows the selection. The documentation link opens the matching language at `https://docs.delino.io/` or `https://docs.delino.io/ko/`. Product screens and API clients require subsequent implementation.

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

Cloudflare Pages builds the `delino-site` workspace from the repository root with `pnpm --filter delino-site build` and publishes `apps/delino-site/dist`. Include repository `LICENSE` and `NOTICE` in distributions. Domain binding and deployment require separate platform configuration; a local build does not establish deployment acceptance.

Production publication uses `.github/workflows/release-delino.yml` after `release-project.yml` commits both Delino app versions and creates the shared `delino@vMAJOR.MINOR.PATCH` tag. Both apps deploy from the same source revision. The owning `delino-site-production` environment supplies Cloudflare configuration. Existing immutable tags can be dispatched again from main for deployment recovery. Follow the [release procedure](repository-workflow-contract.md#web-publication); local builds do not establish Cloudflare or domain acceptance.

## Dependencies and Integrations

Use the repository-pinned React, Rsbuild and TypeScript versions. The marketing site is independent of application runtime and billing services.

## Change Triggers

Keep the project index, workspace registration and workflow contract aligned when paths or validation procedures change. Update instructions only for development procedures, ownership or repository/domain rules.

## References

- [Project index](project-delino-site.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Delcore](project-delcore.md)
