# Project: DevBase

## Goal

Provide a console and the API services of DevBase. The first API service is a free, project-scoped end-user directory that developers manage for their own applications.

## Project ID

`devbase`. The npm workspace is `devbase`.

## Domain Ownership Map

- `servers/crates/delino-protos`: FSL-1.1-ALv2 Rust protocol crate that generates bindings from the owning schemas at Cargo build time.
- `servers/Cargo.toml` and `servers/crates/`: independent server Rust workspace and shared server modules for incremental ports; see the [server Rust workspace contract](servers-rust-workspace-contract.md).

- `servers/scripts/dev-api.mjs`: shared API development launcher.
- `protos/scripts/`: shared Buf entry resolution and protocol breaking checks.

- `apps/devbase`: independently built web application served at `https://devbase.delino.io`.
- `servers/devbase-api`: independent Rust API server for projects, project API keys and end users, with PostgreSQL migrations, maintenance jobs and OCI packaging.
- `protos/devbase/v1`: Connect schema, generated Go and TypeScript messages and Connect-Go bindings.
- `examples/devbase-client` and `examples/devbase-client-ts`: Go and TypeScript consumer examples.
- `.github/workflows/release-devbase.yml`: tagged Cloudflare Pages publication. The `delino` release also deploys this application and publishes the `devbase-api` image. Shared version preparation follows the [workflow contract](repository-workflow-contract.md#project-release-preparation).

## Domain Contract Documents

- [Web application](apps-devbase-web-contract.md)
- [API server](servers-devbase-api-contract.md)
- [Connect protocol](protos-devbase-v1-contract.md)

## Cross-Domain Invariants

Shared UI primitives are owned by [the UI package](project-ui.md) at `packages/ui`.

DevBase API services run as separate servers; the console does not host them. Delcore owns tenants, memberships, billing and tenant lifecycle. The end-user directory is free: it reads only billing-independent tenant state from Delcore and never charges, meters or gates by plan or balance. End users in a directory are developer-owned data without Delino accounts; their developer's application owns their login. Server files use FSL-1.1-ALv2; protocol and example files use Apache-2.0. Individual API services define their server paths and protocols in their own contracts before implementation.

The frontend uses TypeScript, React and Rsbuild instead of the default new-project Go language. Static hosting targets Cloudflare Pages.

## Change Policy

Update this index and the affected web, server and protocol contracts together when paths, interfaces or deployment boundaries change. Follow the [instruction-update policy](README.md#instruction-update-policy). Record validation and unresolved limits in commits, PRs or CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delcore](project-delcore.md)
- [Environment contract](repository-environment-contract.md)
