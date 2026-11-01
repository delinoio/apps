# Project: AnyDash

## Goal

Give solo business owners one dashboard for AI coding subscription usage, local AI session usage merged from many devices, and deployment platform status, usage and cost. Today users check each provider's dashboard or CLI separately, and local session usage stays on each device. AnyDash provides personal dashboards and shared team dashboards on Delcore tenants, with in-app widgets and native widgets. Teams can share selected widgets without sharing the underlying credentials.

AnyDash ships as a web app, macOS and Windows desktop apps, iOS and Android apps and a CLI collector. The first release is a free, general public release with no paid plans or payment features. There is no fixed deadline: release happens after the full agreed scope and every release gate in the domain contracts pass. No feature flag or staged rollout is used, because the whole new service launches together; authentication, tenant authorization and data-access checks stay enforced on their own. Success is defined only by the release gates. Product analytics and success metrics are intentionally not collected.

## Project ID

`anydash`. The app workspace is `@apps/anydash`, the server workspace is `@servers/anydash-api`, the CLI executable is `anydash` and its npm installer is `@delinoio/anydash`. The application identifier is `io.delino.anydash`. The protocol package is `anydash.v1`.

## Domain Ownership Map

Paths marked `planned` are not implemented yet. Planned paths are requirements, not evidence of implementation. Create each path skeleton and its explicit workspace membership in the change that starts its runtime implementation.

- `apps/anydash` (`implemented`): shared ReactLynx client, the browser host for `anydash.delino.io`, the macOS and Windows Lynxtron desktop hosts, the iOS and Android Lynx SDK hosts and the native widget hosts, including separate native widget hosts when Lynxtron packaging cannot embed a widget extension.
  - `apps/anydash/src`: shared ReactLynx application.
  - `apps/anydash/web`: browser host using Lynx for Web, deployed to Cloudflare Pages.
  - `apps/anydash/native/desktop`: macOS and Windows Lynxtron main process and native modules.
  - `apps/anydash/native/ios` and `apps/anydash/native/android`: mobile hosts and their home screen widget extensions.
  - `apps/anydash/native/widgets`: separate native widget hosts, because the Lynxtron packaging cannot embed them: `macos` (a Swift WidgetKit extension in a containing app), `windows` (a C# Widgets Board provider in an MSIX package), and `shared` with `fixtures` (the shared Swift decoder and view and the decoder fixtures generated from the TypeScript bindings).
  - `apps/anydash/scripts`: app-local build and development helpers.
- `cmds/anydash` (`implemented`): Rust `anydash` executable, which is both the CLI and the single shared collector process with its local IPC service and provider adapters. It is a member of the root Cargo workspace and uses Apache-2.0.
- `servers/anydash-api` (`implemented`): Rust Connect API and background worker in one process, PostgreSQL migrations, OCI packaging and reusable operator assets under `servers/anydash-api/ops/`. It is a member of the independent server Cargo workspace and uses FSL-1.1-ALv2.
- `protos/anydash/v1` (`implemented`): canonical `anydash.v1` schema with checked-in Go and TypeScript bindings, registered in `buf.gen.yaml` and exported as `@repo/protos/anydash/v1/anydash_pb`. Rust generation runs in `servers/crates/delino-protos` and in the `cmds/anydash` Cargo build output (`build.rs`) from the same schema.
- `packages/anydash` (`planned`): npm `@delinoio/anydash` executable launcher, its platform packages and the `sh` and PowerShell install scripts served from `anydash.delino.io`.

The main-only unified release selects AnyDash web, server image, CLI, desktop and mobile publication from changesets. Private operator automation owns infrastructure, deployment configuration, rollout and deployment-specific monitoring settings. This repository owns server source, explicit migrations, OCI packaging, image publication and reusable monitoring assets.

## Domain Contract Documents

- [Application, platforms, dashboards, widgets and UX](apps-anydash-foundation.md)
- [Connect protocol](protos-anydash-v1-contract.md)
- [API server, persistence, worker and operations](servers-anydash-api-contract.md)
- [CLI and shared collector](cmds-anydash-cli-contract.md)
- [Provider adapters and metric rules](cmds-anydash-provider-adapters-contract.md)
- [Distribution](packages-anydash-distribution-contract.md)

## Cross-Domain Invariants

### Hosts and local development

- Web: `https://anydash.delino.io` on Cloudflare Pages. API: `https://anydash.api.delino.io`.
- Local development: the `anydash` Portless route (`https://anydash.localhost`) uses fixed loopback port `127.0.0.1:46330`, and the `anydash-api` route (`https://anydash-api.localhost`) uses fixed loopback port `127.0.0.1:46331`. Both fail on port conflicts. The Compose database service is `anydash-postgres` on loopback port `${ANYDASH_POSTGRES_PORT:-55439}`. The Infisical development folder for the API is `/anydash-api`. The API route, database and Infisical folder are registered in the [workflow](repository-workflow-contract.md#anydash-development-and-validation) and [environment](repository-environment-contract.md#anydash-development-environment) contracts; register the web route when the application runtime begins.

### Identity, spaces and authorization

- Logto owns identity and sign-in on every platform. Delcore owns users, tenants and the Owner, Admin, Billing and Member roles. The personal space is the user's Delcore personal tenant; team spaces are Delcore organization tenants. AnyDash never reads Delcore tables and never redefines tenants or roles.
- AnyDash identifies users by the Delcore internal user ID and spaces by the Delcore tenant ID. A collector device belongs to the Logto-authenticated user who approved its enrollment.
- Authorization and tenant isolation are enforced on every RPC, apart from the data-owner sharing rules. Only Delcore tenant state `ACTIVE` permits activity; `BLOCKED`, `DELETING`, `DELETED` and `UNKNOWN` deny it, and only explicit `DELETED` permits tenant-lifecycle cleanup.
- Team role matrix: Owner and Admin create, delete, edit and view team dashboards; Member edits and views; Billing-only users and non-members have no access. Only the data owner changes a shared widget's query period, filters and sharing scope.
- The role matrix needs each caller's fixed Delcore roles. `ListMembers` serves only Owners and Admins, so AnyDash uses the additive, service-scoped Delcore role read `GetTenantMemberRoles`, defined in the Delcore protocol and server contracts without changing existing tenant, role or retention semantics.

### Data and protocol

- Connect RPC is the business interface. Web and app clients use `@connectrpc/connect-query` with React Query. The local collector exposes the `anydash.v1.LocalCollectorService` over local IPC only.
- Identifiers of AnyDash-owned entities are UUID v7.
- External provider credentials stay on the collector device and are never uploaded. The server receives only aggregates per device, tool, model and 15-minute UTC bucket, plus external metric snapshots. Per-session records, prompts, response bodies and per-project information are never uploaded.
- Every error carries a correlation ID. The server stays backward compatible, and Buf breaking checks apply to `protos/anydash/v1`. The operator sets minimum supported client versions; older clients receive "update required" and keep aggregating locally for up to 90 days.
- AnyDash never refreshes a CLI's stored OAuth token.

### Platforms and deviations from repository defaults

- Clients are one shared ReactLynx application, English only, with a user-selected time zone. The desktop hosts use Lynxtron on macOS and Windows; mobile hosts use the Lynx SDKs. TypeScript/ReactLynx and platform integration languages are the UI exception to the repository language defaults.
- The collector process and the CLI are written in Rust instead of the repository's Go default for non-server projects, so that the CLI, the desktop app's collector and the provider adapters share one implementation in the root Cargo workspace.
- Native widget hosts may use Swift, C++/WinRT, Kotlin or other platform languages when Lynxtron or the Lynx SDKs cannot host them.
- There is no Linux native GUI and no Plasma widget in the first release. Linux users get the CLI collector and the web app. There is no Windows arm64 app unless Lynxtron supports a native arm64 build; those users get the CLI and the web app.
- The server is Rust under `servers/` with FSL-1.1-ALv2. Client, protocol, CLI and distribution code outside `servers/` uses Apache-2.0. Third-party licenses and notices are kept.
- PostgreSQL is the server source of truth. No file storage is used, so the Cloudflare R2 default does not apply. No AI search is used.

### Environments and recovery

- Environments are local development (Compose database and mocked providers) and production. There is no staging environment.
- Recovery is fix-forward only, with explicit forward migrations. No SLO is set; the service is best effort.
- Support uses public GitHub Issues handled by the maintainer, with no response-time commitment. Security reports use GitHub private vulnerability reporting. Users are told not to post personal data in public issues. Account data deletion is done in the app.
- Privacy policy and terms of service are a separate legal deliverable that must exist before store publication.

### Release gates

Release happens only after every gate passes:

- Automated tests, mocked-provider tests, per-OS packaging checks and the load test (1,000 concurrent active users and 5,000 collector devices) pass, as defined in the domain contracts. These do not replace the real-account and real-device gates or store acceptance.
- Maintainer-performed real-account and real-device validation: a real-account connection for every provider; app and native widget runs on each supported OS; aggregation from 2 or more devices and automatic collection-device failover; team permissions and share revocation; a real check of each "Not provided" metric.
- Non-regression: Delcore tenant, role and retention contracts unchanged (the role read is additive), and existing CI macOS-runner restrictions unchanged outside the AnyDash distribution gates.
- The alerts, correlation IDs, activity log, local log rotation and public documentation pages exist as specified.

### Out of scope

- Paid plans, billing and payment features; a feature flag or staged rollout; product analytics, success metrics and an SLO; a staging environment.
- Linux native GUI and Plasma widgets; a Windows arm64 app if Lynxtron does not support it.
- Custom widget types; editing from native widgets.
- Deployment write operations (redeploys and similar), deployment history, incidents and logs.
- Unofficial collection through browser sessions or cookies; AnyDash refreshing CLI OAuth tokens; an automatic Claude Code fallback path (for example a statusline hook) if `/api/oauth/usage` becomes unavailable.
- Supabase cost, including configured add-on prices; plan quotas that providers publish only in documentation; xAI Management API spend for API keys.
- Cross-device deduplication of copied session files; per-session or per-project data upload; data recovery beyond 90 days.
- Currency conversion; localization beyond English.
- A threat-model document or penetration test; the security review is ordinary code review.
- Privacy policy and terms of service, which are a separate legal deliverable required before store publication.
- Automatic store submission; automatic encryption key rotation; a CLI self-update command; an APT repository or `.deb` packages.

## Change Policy

Update this index, the affected domain contracts and the [documentation catalog](README.md) together when paths, platforms, protocols, hosts, ports or deployment boundaries change. Change the protocol, server, CLI, provider adapter and application contracts together when wire fields, authorization, collection or retention rules change. Mark a path as implemented only in the change that adds its runtime implementation. Update `AGENTS.md` only when development procedures, directory ownership or repository/domain development rules change; follow the [instruction-update policy](README.md#instruction-update-policy). Record new implementation status, validation results and unresolved limits in pull requests, issues and CI logs/artifacts. Do not treat planned contracts or mocked-provider tests as evidence of real-account, native or store acceptance.

## References

- [Repository defaults](repository-defaults.md)
- [Repository boundary](repository-boundary-contract.md)
- [Repository license](repository-license-contract.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository environment](repository-environment-contract.md)
- [Changeset contract](repository-changeset-contract.md)
- [Server Rust workspace](servers-rust-workspace-contract.md)
- [Delcore project](project-delcore.md)
- [Lane project](project-lane.md)
- [Lynx UI](project-lynx-ui.md)
- [Delino Docs](project-delino-docs.md)
