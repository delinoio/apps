# DevBase Web Application

## Scope

`apps/devbase` owns the web application at `https://devbase.delino.io`. Provide a console for managing the API services that belong to DevBase.

## Runtime and Language

TypeScript, React 19 and Rsbuild follow the repository frontend stack. Node.js 24 runs development and build tooling. Cloudflare Pages serves the static build.

## Users and Operators

DevBase API service users manage services through the console. Maintainers build and publish the web application.

## Interfaces and Contracts

The root route `/` has the title `DevBase`. The scaffold renders an empty React root. Product screens and API clients require subsequent implementation.

DevBase API services run as separate servers. They use delcore for billing; the console does not host these services or own billing. Individual API service names, server paths and protocols must be defined in their own contracts before implementation.

Run package-local `pnpm dev` on `127.0.0.1:46313`. A port conflict must stop startup. The production domain does not replace the loopback development address.

## Storage

The scaffold has no persistent data, cookies or account state. `dist` is ignored generated build output.

## Security

The browser bundle must contain no service credentials. Deployment credentials remain outside the repository. Future paid execution must use server-side delcore authorization and metering under the Delcore contracts.

## Logging

Rsbuild reports startup and build failures. Future service integrations must emit structured failure events without credentials or raw user content.

## Build and Test

Run `pnpm test` in this directory for TypeScript checks and a production build. `pnpm build` emits `dist`. Affected CI runs these checks on Ubuntu. Remove generated `dist` after validation.

Cloudflare Pages builds the `devbase` workspace from the repository root with `pnpm --filter devbase build` and publishes `apps/devbase/dist`. Include repository `LICENSE` and `NOTICE` in distributions. Domain binding and deployment require separate platform configuration; a local build does not establish deployment acceptance.

Production publication uses `.github/workflows/release-devbase.yml` after `release-project.yml` commits and tags the selected version. The owning `devbase-production` environment supplies Cloudflare configuration. Existing immutable tags can be dispatched again from main for deployment recovery. Follow the [release procedure](repository-workflow-contract.md#web-publication); local builds do not establish Cloudflare or domain acceptance.

## Dependencies and Integrations

Use the repository-pinned React, Rsbuild and TypeScript versions. Delcore owns billing for the separate DevBase API services; no live integration is part of the scaffold.

## Change Triggers

Keep the project index, workspace registration and workflow contract aligned when paths or validation procedures change. Update instructions only for development procedures, ownership or repository/domain rules.

## References

- [Project index](project-devbase.md)
- [Repository defaults](repository-defaults.md)
- [Environment ownership](repository-environment-contract.md)
- [Delcore](project-delcore.md)
