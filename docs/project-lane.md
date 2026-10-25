# Project: Lane

## Goal

Provide repository-scoped task and prompt management for individuals and their existing agents. Native clients and the CLI work without an account or server. Optional synchronization connects devices; the web client requires an online account. Lane does not execute agents or call AI providers.

## Project ID

`lane`. The app workspace is `lane`; the application identifier remains `io.delino.lane`. The CLI installer is `@delinoio/lane`.

## Domain Ownership Map

Existing paths retain their build ownership:

- `apps/lane/src`: shared ReactLynx application.
- `apps/lane/web`: browser host using Lynx for Web.
- `apps/lane/native/android`: Android SDK host and Gradle project.
- `apps/lane/native/ios`: iOS SDK host and XcodeGen project specification.
- `apps/lane/native/desktop`: macOS and Windows Lynxtron main process, Node-API host module and CMake project.
- `apps/lane/scripts`: app-local build and development helpers.

Additional component ownership:

- `apps/lane/native/store`: shared SQLite schema, native store support and bridge domain operations; platform adapters remain in the existing hosts.
- `cmds/lane`: Go CLI entry point and local stdio MCP.
- `cmds/lane/internal`: Go local store, domain rules, synchronization and CLI/MCP adapters in the root Go module.
- `protos/lane/v1`: canonical schemas, checked-in Go/TypeScript bindings and shared cross-language fixtures. Rust generation remains in `servers/crates/delino-protos` Cargo output.
- `servers/lane-api`: Rust Connect API, remote MCP, webhook intake, PostgreSQL migrations, push workers, OCI packaging and operator assets in the independent server workspace.
- `packages/lane`: npm executable installer, common agent `SKILL.md`, installation and MCP connection guides.

## Domain Contract Documents

- [Application and full product behavior](apps-lane-foundation.md)
- [Protocol, domain rules and conformance vectors](protos-lane-v1-contract.md)
- [Shared local store and native bridge](apps-lane-local-store-contract.md)
- [Go CLI and local MCP](cmds-lane-cli-contract.md)
- [Server, synchronization, identity and operations](servers-lane-api-contract.md)
- [Distribution and common agent skill](packages-lane-distribution-contract.md)

## Cross-Domain Invariants

Native apps target macOS, Windows, iOS and Android. Linux supports CLI/local MCP and online web, not a native GUI. Keep ReactLynx and Rspeedy for the shared client, Lynx SDKs for mobile, and Lynxtron for desktop. TypeScript/ReactLynx and platform integration languages remain the UI exception to repository language defaults; new CLI code uses Go and server code uses Rust.

Native hosts and Go CLI share the specified SQLite format on desktop. They implement the same versioned domain rules and run the same fixture vectors. Native business logic runs in the host, not in ReactLynx. Web reads/writes the server only, without an offline mutation queue or independent browser database. PostgreSQL owns server state; local SQLite owns standalone work and durable pending synchronization changes.

Connect is the business interface. Local Native Modules, CLI, stdio MCP, remote MCP and webhooks are adapters to the same typed commands and invariants. Use UUID v7 entity, request and version identities. Preserve whole-task conflicts, history, deletion precedence and replay protection across languages. Never use wall-clock last-write-wins.

Logto owns online identity. `lane-api` depends on Delcore Connect for personal-tenant authorization; it does not read Delcore tables or redefine tenants. Local use needs neither service. All repositories synchronize within one personal account; notification preferences remain per device. There are no teams, billing or feature flags.

Web is served at `lane.delino.io`; API and remote MCP at `lane.api.delino.io`. Deployable artifacts are required, but deployment, real external-provider acceptance and formal/store publication are separate. Native build/install/behavior acceptance remains required; a shared bundle build is not native acceptance.

Repository-owned server files/distributions use FSL-1.1-ALv2. Other repository-owned files/distributions use Apache-2.0. Preserve upstream SDK notices. Keep protocol and runtime consumers in one repository and review.

## Change Policy

Update this index, affected domain contracts and catalog together when paths, formats, platforms or deployment boundaries change. New ownership routes belong in scoped instructions; product behavior stays in contracts. Do not treat planned contracts as evidence of implementation. Record source revisions, commands, actual results and remaining native/provider acceptance in PRs and CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository boundary](repository-boundary-contract.md)
- [Repository workflow](repository-workflow-contract.md)
- [Delcore project](project-delcore.md)
- [Lynx UI](project-lynx-ui.md)
