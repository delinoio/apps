# FixKey

## Goal

Correct words typed with the wrong keyboard layout without requiring users to switch input languages. For each completed word, FixKey reconstructs candidates under every enabled layout, validates each candidate against its language dictionary, and replaces the word only when exactly one enabled language accepts its candidate. The first version supports Korean Dubeolsik and US English QWERTY in both directions on a personal, local desktop application. There is no fixed release deadline.

## Project ID

`fixkey`

FixKey is a standalone local desktop product owned by `https://github.com/delinoio/apps`. It does not depend on another product or a server.

## Domain Ownership Map

`crates/fixkey-core` and `apps/fixkey-app/src-tauri` are explicit Rust workspace members. `apps/fixkey-app` owns the desktop shell and frontend.

- `crates/fixkey-core` (`core`): Rust engine for layout reconstruction, language validation, correction and undo state, and platform accessibility adapters. It has no UI dependency.
- `apps/fixkey-app` (`desktop`): Tauri 2 desktop shell with a React and Rsbuild settings UI. The Rust shell lives in `apps/fixkey-app/src-tauri`. This component also owns `.github/workflows/release-fixkey.yml` and the `scripts/release/fixkey*` artifact, license and release-contract helpers.

## Domain Contract Documents

- [Rust correction engine contract](crates-fixkey-core-foundation.md)
- [Desktop application contract](apps-fixkey-desktop-contract.md)

## Cross-Domain Invariants

Shared UI primitives are owned by [the UI package](project-ui.md) at `packages/ui`.

- Component identifiers remain stable: `core`, `desktop`.
- Supported language identifiers remain stable: `ko-dubeolsik`, `en-us-qwerty`.
- Target platform identifiers remain stable: `macos-arm64`, `windows-x64`, `windows-arm64`, `linux-x11-x64`, `linux-x11-arm64`.
- Raw input handling and morphology processing stay in Rust. The React UI never receives input text except candidate text shown in the manual picker for the active context.
- Input text, reconstructed candidates, document content, window titles and correction history are never persisted or transmitted. Network communication is limited to application update checks and downloads.
- Target platforms are not verified application compatibility. Documentation and release notes must keep the two distinct, and must not claim real-user accuracy or application compatibility from fixtures, mocks or builds.
- The project provides no public API, account service, backend, billing, synchronization or mobile application. Rust, Tauri and local storage are explicit exceptions to the repository language and storage defaults in [repository defaults](repository-defaults.md).
- Repository-owned code is Apache-2.0. Pinned third-party dictionaries and morphology data keep their upstream licenses and notices in every distribution.

Shared version preparation follows the [workflow contract](repository-workflow-contract.md#project-release-preparation). It increments the desktop application's npm, Tauri and native package versions and corresponding lock entry together, uses the updater's `fixkey@v` tag namespace and leaves the engine version independent. The tag starts the owning release workflow; signing and publication use its protected environments. Native acceptance is recorded separately when performed.

## Change Policy

Update this index and the owning domain contracts when paths, component identifiers, supported layouts, platforms or cross-domain boundaries change. Update `AGENTS.md` only when development procedures, directory ownership or repository/domain development rules change, following the [instruction-update policy](README.md#instruction-update-policy). Record implementation status, source revisions, validation commands, results and unperformed checks (actual OS and application execution, installation-package inspection) in pull requests, issues and CI artifacts.

## References

- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)
- [Dependency security contract](repository-dependency-security-contract.md)
- [Project index template](project-template.md)
- [Domain contract template](domain-template.md)
