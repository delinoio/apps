# DevBase Web Application

## Scope

`apps/devbase` owns the web application at `https://devbase.delino.io`. Provide a console for managing the API services that belong to DevBase.

## Runtime and Language

TypeScript, React 19 and Rsbuild follow the repository frontend stack. Node.js 24 runs development and build tooling. Cloudflare Pages serves the static build.

## Users and Operators

DevBase API service users manage services through the console. Maintainers build and publish the web application.

## Interfaces and Contracts

The root route `/` has the title `DevBase`. The scaffold renders an empty React root. Product screens and API clients require subsequent implementation.

DevBase API services run as separate servers; the console does not host them or own billing. Delcore owns tenants and billing. The end-user directory API is free and billing-independent: it never charges, meters or gates by plan or balance, so the console must not add billing behavior for it. Other API services use delcore for billing under their own contracts. Individual API service names, server paths and protocols must be defined in their own contracts before implementation.

Run package-local `pnpm dev` on `127.0.0.1:46313`. A port conflict must stop startup. The production domain does not replace the loopback development address.

## Storage

The scaffold has no persistent data, cookies or account state. `dist` is ignored generated build output.

## Security

The browser bundle must contain no service credentials. Deployment credentials remain outside the repository. Future paid execution must use server-side delcore authorization and metering under the Delcore contracts.

## Logging

Rsbuild reports startup and build failures. Future service integrations must emit structured failure events without credentials or raw user content.

## Build and Test

Run `pnpm test` in this directory for TypeScript checks and a production build. `pnpm build` emits `dist`. Affected CI runs these checks on Ubuntu. Remove generated `dist` after validation.

Cloudflare Pages builds the `@apps/devbase` workspace from the repository root with `pnpm --filter @apps/devbase build` and publishes `apps/devbase/dist`. Include repository `LICENSE` and `NOTICE` in distributions. Domain binding and deployment require separate platform configuration; a local build does not establish deployment acceptance.

Production publication uses `.github/workflows/release.yml` after the unified workflow commits changeset consumption and selected versions, publishes server images and receives an authorized publication dispatch after operator deployment verification. The repository-wide `delino-site-production` environment supplies Cloudflare configuration, with optional Pages project override `CLOUDFLARE_PAGES_PROJECT_DEVBASE`. Use the prepared commit SHA and successful preparation run ID in an authorized release-publish event for publication recovery. Follow the [release procedure](repository-workflow-contract.md#web-publication); local builds do not establish Cloudflare or domain acceptance.

## Dependencies and Integrations

`@packages/ui` supplies optional shared components and PostCSS configuration. Follow [the shared UI contract](packages-ui-foundation.md) for stylesheet imports and component adoption.

Use the repository-pinned React, Rsbuild and TypeScript versions. Delcore owns tenants and billing for the separate DevBase API services. The end-user directory is free and billing-independent. No live integration is part of the scaffold.

## Change Triggers

Keep the project index, workspace registration and workflow contract aligned when paths or validation procedures change. Update instructions only for development procedures, ownership or repository/domain rules.

## References

- [Project index](project-devbase.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Delcore](project-delcore.md)
