# Lane Distribution Contract

## Scope

`packages/lane` owns the npm installer `@delinoio/lane`, executable manifests/checksums, common agent `SKILL.md`, CLI installation and MCP connection guides. CLI source remains in `cmds/lane`; native app build ownership remains in `apps/lane`; server OCI/runtime/runbooks remain in `servers/lane-api`. These distribution paths do not establish package publication.

## Runtime and Language

The installer is a small Node.js launcher/package; the product CLI is Rust. Follow pinned repository Node/pnpm tooling and Rstack formatting. Produce Rust executables for macOS, Windows and Linux, initially amd64 and arm64 where the executable dependencies support those targets. CLI architecture support does not imply a native GUI SDK for the same architecture. In particular, do not infer Windows ARM64 GUI support from a Rust cross-build.

## Users and Operators

Individuals install the CLI directly or through npm; agents consume the common skill and connect to user-approved MCP. Operators use deployable server/web artifacts and runbooks. Store publication, formal native distribution and production deployment remain follow-up operations.

## Agent Skills Distribution

Skills that help agents use Lane will be distributed through [delinoio/lane-skills](https://github.com/delinoio/lane-skills). This is the planned skills distribution repository; this contract does not establish that the skills have been published. The common `SKILL.md` requirements below remain applicable.

## Interfaces and Contracts

`npm install -g @delinoio/lane` installs a `lane` executable launcher. Direct binary archives use `lane-<os>-<arch>` identities (`darwin`, `windows`, `linux`; Windows payload `lane.exe`, others `lane`). Include version/platform manifests, checksums, repository license and notices. The npm launcher declares exact-version `optionalDependencies` on `@delinoio/lane-<os>-<arch>` packages for all six supported targets. Each platform package contains only its matching executable, manifest/checksum and licenses; its npm `os` and `cpu` fields select the host (`win32` for Windows and `x64` for amd64). The launcher package contains no executables. The npm launcher resolves the installed platform package, verifies its name/version/OS/CPU and the launcher manifest/checksum and propagates arguments, stdio, signals and exit codes. Unsupported platforms or missing/mismatched binaries fail with a useful installation error; never fall back to unverified execution or a server-only command.

Prepare archives and npm packages for review with reproducible build instructions and executable smoke checks. Keep optional dependencies enabled for npm installation. Missing platform packages fail with a reinstall instruction; do not download or compile a fallback during installation or normal command execution. Publish matching platform packages before the launcher; packaging tests do not publish them. Do not publish a package or release merely to run packaging tests.

The common `SKILL.md` must explain Lane task/proposal/progress distinctions, optional prompts, explicit save/history/restore, whole-task conflicts, deletion confirmation, local versus remote data, webhook event leases and completion, and online-only operations. It must instruct agents to read and manage user work through MCP without pretending that Lane executes agents. Preserve full management tools; omit decision-item features and mini-app implementations.

Ship connection examples:

- Local MCP: executable `lane`, arguments `["mcp", "serve"]`; account-free SQLite data shared with the native app/CLI. Explain OS-user boundary and offline behavior.
- Remote MCP: `https://lane.api.delino.io/mcp`, browser Logto approval, full-account access, connection listing and individual revocation. Explain that this endpoint remains available when devices are off.
- CLI local default and explicit `--remote`; human output versus `--json`; request IDs and expected versions for safe retries.

User guides cover installation, repository registration/rename/moves, task/proposal management, prompt restore, conflicts, explicit Save and failure/input retention, login/sync/logout choices, device/connection management, all three deletion scopes, webhook settings/events and processing, per-device notifications and unsupported push environments. Describe `lane-dev://` versus `lane://` callback/open/repository/task links. No decision-item links are included. Public docs show supported behavior and stable public interfaces, not repository-internal contracts or operational secret paths.

Native distributions retain the existing SDK, embedded production-bundle and license-notice contracts. A desktop package must make its matching `lane sync run` helper available to host supervision without assuming a globally installed npm package. Register deep-link routes by build channel; queue cold-launch routing until storage/UI listeners are ready. Formal signing/store setup is separate from unsigned build/install/behavior validation. Do not add WNS registration to Windows packaging.

## Desktop Chrome companion packaging

The existing Lane desktop package includes the standalone Native Messaging relay,
its private pinned Node 24 runtime and the original Node license. macOS packages
also retain a patched Node 22 compatibility runtime and its license for macOS 12
through 13.4, where the official Node 24 binary cannot run. The launcher selects
the runtime for the host OS and connects to an already running Lane host. It must not launch
Lane. The build records the runtime version and verified archive checksum; this
integrity check does not replace dependency security review.

The desktop installer registers the exact installed relay path for the explicitly
configured extension IDs. An empty ID list disables registration and connection
service. Uninstall removes only the matching registration and preserves Lane data.
Use the [application packaging procedure](apps-lane-foundation.md#desktop-capture-packaging-and-local-validation)
for local builds and isolated installer checks. Fixture IDs and unsigned packages
do not establish Chrome Web Store publication or native runtime acceptance.

## Storage

All `dist` directories are ignored generated output and must be removed from the final worktree. Generate packages before tests that consume them; never commit built executables, SDKs or release credentials. Assets at least 512 KiB use exact-path LFS attributes when they are legitimate tracked source assets; hydrate before builds/tests. Runtime data uses the shared local-store path, never the installation directory.

## Security

Verify executable source/version/platform and checksums. Keep signing, npm publication, Logto, push and deployment credentials outside Git. Local examples contain no real tokens. Preserve Apache-2.0 for CLI/package/native repository code and FSL-1.1-ALv2 for server distributions, with upstream notices unchanged.

## Logging

Installer/launcher errors identify platform/version and safe failure categories, never user data, credentials or environment dumps. Runtime CLI logging follows its owning contract; package installation must not upload diagnostics.

## Build and Test

Run root install, package-local checks and packed-package smoke tests from a disposable install directory. Use an isolated loopback npm registry serving only local candidate archives to test normal platform selection without public publication or a pre-populated cache. Verify that installation retains only the host platform package and that omitting optional dependencies produces an actionable error. Verify archive contents, checksums, licenses, executable selection, missing/unsupported platform errors, argument/stdin/stdout/stderr/exit propagation, and local stdio MCP startup. Build macOS/Windows/Linux binaries with the matching Rust targets, C linker/native SDK and `protoc`, and record which were actually executed. The default package build requires all six target toolchains; repeated `--platform` selections produce explicitly partial validation artifacts. CI uses the Linux amd64 host subset. Full release candidates must contain all six artifacts. Native SDK build/install/behavior and accessibility acceptance remain separate on all native targets.

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

## RealQA integration

The [RealQA contract](apps-lane-realqa-contract.md) extends this domain with durable
final-PNG attachments, authorized retrieval and synchronized URL-to-repository
rules. Preserve existing account routing, whole-task conflicts, request replay,
history, task moves and deletion precedence. Native saves commit locally before
optional image upload. Pending synchronization must not be reported as failed
local saving. Unsupported schema/format versions must be refused before writes.
These requirements do not establish implementation or release acceptance.
