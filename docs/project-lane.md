# Project: Lane

## Goal

Provide the Lane client application for desktop, mobile and web.

## Project ID

`lane`. The npm workspace is `lane`; the application identifier is `io.delino.lane`.

## Domain Ownership Map

- `apps/lane/src`: shared ReactLynx application.
- `apps/lane/web`: browser host using Lynx for Web.
- `apps/lane/native/android`: Android SDK host and Gradle project.
- `apps/lane/native/ios`: iOS SDK host and XcodeGen project specification.
- `apps/lane/native/desktop`: macOS and Windows Clay hosts and CMake project.
- `apps/lane/scripts`: app-local build and development helpers.

## Domain Contract Documents

- [Application foundation](apps-lane-foundation.md)

## Cross-Domain Invariants

All hosts render the same empty Lane application. Native hosts use Lynx SDK;
desktop uses Clay. The browser uses Lynx for Web. There are no product features,
account flows, servers, RPC schemas or persisted data in this scaffold.
TypeScript and ReactLynx are the language exception for shared UI; Kotlin,
Objective-C and C++/Objective-C++ integrate the platform SDKs. Rspeedy owns the
shared build. Repository-owned code is Apache-2.0; preserve upstream SDK notices.

## Change Policy

Update this index and the application contract together when paths, platforms,
build interfaces or deployment boundaries change. Follow the repository
instruction-update policy. Record validation and unresolved platform acceptance
in PRs and CI evidence.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Lynx UI](project-lynx-ui.md)
