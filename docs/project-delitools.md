# Project: DeliTools

## Goal

Provide useful end-user tools through desktop, mobile and web applications, with usage-based billing through delcore. Retain the desktop, Android and iOS hosts and the mobile distribution pipeline.

## Project ID

`delitools`. The npm workspace is `@apps/delitools`; the native package is `delitools-shell`.

## Domain Ownership Map

- `apps/delitools`: shared React frontend for the web application at `https://delitools.delino.io`, CEF desktop host and Wry mobile host.
- `.github/workflows/release.yml`: changeset-selected Cloudflare Pages web publication through shared `delino-site-production`. Shared version preparation follows the [workflow contract](repository-workflow-contract.md#project-release-preparation).
- `apps/delitools/scripts/mobile-workflow.test.mjs`: fixture checks for the mobile release workflow.
- `.github/workflows/deploy-delitools-mobile.yml`: signed Android internal testing and iOS TestFlight uploads.
- `servers/delitools-api`: independent Rust API for paid translation and extraction, with PostgreSQL migrations, a usage-delivery worker and OCI packaging. The unified release publishes its image when selected; see the [workflow contract](repository-workflow-contract.md#server-image-publication).
- `protos/delitools/v1`: Connect schema, generated Go and TypeScript messages and Connect-Go bindings.
- `examples/delitools-client`: Go consumer example.
- `servers/crates/delino-protos` and `servers/scripts/dev-api.mjs`: shared Rust protocol crate and API development launcher; see the [server Rust workspace contract](servers-rust-workspace-contract.md).
- `apps/delino-docs`: owns the public DeliTools API pages under `/services/delitools/`; internal contracts stay in `docs/`.

## Domain Contract Documents

- [Application foundation](apps-delitools-foundation.md)
- [API server](servers-delitools-api-contract.md)
- [Connect protocol](protos-delitools-v1-contract.md)

## Cross-Domain Invariants

Shared UI primitives are owned by [the UI package](project-ui.md) at `packages/ui`.

All hosts render an empty root with the `DeliTools` application name and `io.delino.delitools` identifier. The application scaffold has no account access, RPC or product storage, and clients do not own billing or service credentials. The web build targets Cloudflare Pages. DeliDev remains a separate application. React, Rsbuild and Rust follow the existing DeliDev shell instead of the default new-project Go language. Mobile uses system webviews; CEF is desktop-only.

The DeliTools API server is a separate deployable. It provides synchronous Connect RPC methods for text-array translation and text-to-JSON extraction to external developers, runs models through Vercel AI Gateway, and uses delcore for shared API keys, authorization and token billing. Delcore stays service-neutral: it knows DeliTools only as a registered service with capabilities, meters, prices and events, and contains no DeliTools-, Gateway-, translation-, extraction- or model-specific logic. Delcore owns the reusable personal and tenant API keys and their authorization; DeliTools owns Gateway calls, the model allowlist, provider usage interpretation and the mapping from models and token directions to generic catalog meters. The server stores no submitted text, schema or result; it keeps only a usage-delivery outbox that expires 30 days after first creation. Submitted content is sent to Vercel AI Gateway and its model provider, and local non-persistence is not a provider retention guarantee. The server uses Rust, PostgreSQL and one process per database, and its files use FSL-1.1-ALv2; the protocol uses Apache-2.0. Server production deployment and live-account acceptance are separate from repository validation. The [server contract](servers-delitools-api-contract.md) and [protocol contract](protos-delitools-v1-contract.md) own the behavior and wire types.

## Change Policy

Update this index and the affected application, server and protocol contracts together when paths, targets, interfaces or release boundaries change. Keep validation results and unperformed native/store checks in commit, PR or CI records. Update instructions only for ownership and development-rule changes.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [DeliDev](project-delidev.md)
- [Delcore](project-delcore.md)
- [Environment contract](repository-environment-contract.md)
