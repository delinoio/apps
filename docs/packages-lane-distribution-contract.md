# Lane Distribution Contract

## Scope

`packages/lane` owns the npm installer `@delinoio/lane`, executable manifests/checksums, common agent `SKILL.md`, CLI installation and MCP connection guides. CLI source remains in `cmds/lane`; native app build ownership remains in `apps/lane`; server OCI/runtime/runbooks remain in `servers/lane-api`. These distribution paths do not establish package publication.

## Runtime and Language

The installer is a small Node.js launcher/package; the product CLI is Rust. Follow pinned repository Node/pnpm tooling and Rstack formatting. Produce Rust executables for macOS, Windows and Linux, initially amd64 and arm64 where the executable dependencies support those targets. CLI architecture support does not imply a native GUI SDK for the same architecture. In particular, do not infer Windows ARM64 GUI support from a Rust cross-build.

## Users and Operators

Individuals install the CLI directly or through npm; agents consume the common skill and connect to user-approved MCP. Operators use deployable server/web artifacts and runbooks. Store publication, formal native distribution and production deployment remain follow-up operations.

## Interfaces and Contracts

`npm install -g @delinoio/lane` installs a `lane` executable launcher. Direct binary archives use `lane-<os>-<arch>` identities (`darwin`, `windows`, `linux`; Windows payload `lane.exe`, others `lane`). Include version/platform manifests, checksums, repository license and notices. The npm launcher resolves its packaged platform artifact, verifies the manifest/checksum and propagates arguments, stdio, signals and exit codes. Unsupported platforms or missing/mismatched binaries fail with a useful installation error; never fall back to unverified execution or a server-only command.

Prepare archives and npm packages for review with reproducible build instructions and executable smoke checks. An installer download, if used, must use an immutable release version and verified checksum; never fetch a mutable latest binary during normal command execution. Do not publish a package or release merely to run packaging tests.

The common `SKILL.md` must explain Lane task/proposal/progress distinctions, optional prompts, explicit save/history/restore, whole-task conflicts, deletion confirmation, local versus remote data, webhook event leases and completion, and online-only operations. It must instruct agents to read and manage user work through MCP without pretending that Lane executes agents. Preserve full management tools; omit decision-item features and mini-app implementations.

Ship connection examples:

- Local MCP: executable `lane`, arguments `["mcp", "serve"]`; account-free SQLite data shared with the native app/CLI. Explain OS-user boundary and offline behavior.
- Remote MCP: `https://lane.api.delino.io/mcp`, browser Logto approval, full-account access, connection listing and individual revocation. Explain that this endpoint remains available when devices are off.
- CLI local default and explicit `--remote`; human output versus `--json`; request IDs and expected versions for safe retries.

User guides cover installation, repository registration/rename/moves, task/proposal management, prompt restore, conflicts, explicit Save and failure/input retention, login/sync/logout choices, device/connection management, all three deletion scopes, webhook settings/events and processing, per-device notifications and unsupported push environments. Describe `lane-dev://` versus `lane://` callback/open/repository/task links. No decision-item links are included. Public docs show supported behavior and stable public interfaces, not repository-internal contracts or operational secret paths.

Native distributions retain the existing SDK, embedded production-bundle and license-notice contracts. A desktop package must make its matching `lane sync run` helper available to host supervision without assuming a globally installed npm package. Register deep-link routes by build channel; queue cold-launch routing until storage/UI listeners are ready. Formal signing/store setup is separate from unsigned build/install/behavior validation. Do not add WNS registration to Windows packaging.

## Storage

All `dist` directories are ignored generated output and must be removed from the final worktree. Generate packages before tests that consume them; never commit built executables, SDKs or release credentials. Assets at least 512 KiB use exact-path LFS attributes when they are legitimate tracked source assets; hydrate before builds/tests. Runtime data uses the shared local-store path, never the installation directory.

## Security

Verify executable source/version/platform and checksums. Keep signing, npm publication, Logto, push and deployment credentials outside Git. Local examples contain no real tokens. Preserve Apache-2.0 for CLI/package/native repository code and FSL-1.1-ALv2 for server distributions, with upstream notices unchanged.

## Logging

Installer/launcher errors identify platform/version and safe failure categories, never user data, credentials or environment dumps. Runtime CLI logging follows its owning contract; package installation must not upload diagnostics.

## Build and Test

Run root install, package-local checks and packed-package smoke tests from a disposable install directory. Verify archive contents, checksums, licenses, executable selection, missing/unsupported platform errors, argument/stdin/stdout/stderr/exit propagation, and local stdio MCP startup. Build macOS/Windows/Linux binaries with the matching Rust targets, C linker/native SDK and `protoc`, and record which were actually executed. The default package build requires all six target toolchains; repeated `--platform` selections produce explicitly partial validation artifacts. CI uses the Linux amd64 host subset. Full release candidates must contain all six artifacts. Native SDK build/install/behavior and accessibility acceptance remain separate on all native targets.

Keep existing PR/main/affected CI platform restrictions and default affected manual validation. Do not add macOS PR runners or a full manual CI mode. Produce reviewable web/API/MCP deployment outputs, provider configuration instructions, mocked tests, dashboards/alerts and runbooks through their owning domains. Do not claim real Logto/push/email delivery, production deployment, store publication or formal release from artifact tests.

## Dependencies and Integrations

Coordinate CLI, native helper packaging, common protocol versions and user guides in one checkout. Follow repository license, dependency-security, workflow and environment contracts. The package distributes no AI execution service.

## Change Triggers

Change CLI/app/server/project contracts and public user guides together when installation, target architecture, helper lifecycle or supported workflows change. Update scoped instructions only for ownership/development changes.

## References

- [Project](project-lane.md)
- [Repository defaults](repository-defaults.md)
- [CLI](cmds-lane-cli-contract.md)
- [Application](apps-lane-foundation.md)
- [Local store](apps-lane-local-store-contract.md)
- [Repository license](repository-license-contract.md)
