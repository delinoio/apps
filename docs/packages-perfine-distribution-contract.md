# Perfine Action and CLI distribution

## Scope

`actions/perfine` owns Action source and build inputs. `packages/perfine` owns npm package `perfine` and CLI packaging. Publish Action output to `delinoio/perfine-action`; the source remains in this monorepo.

## Runtime and Language

The executable is the Rust CLI from `cmds/perfine`. Node.js packaging and Action tooling follow the root runtime selector and pinned pnpm toolchain. Official npm and standalone CLI hosts follow the four-target [minimum-version policy](project-perfine.md#supported-cli-hosts). Standalone executables require neither npm nor Node.js; the npm launcher requires Node.js 24 or later. Keep release distributions outside tracked directories named `dist`.

## Users and Operators

Local CLI users, repository administrators generating workflows, Action maintainers, release operators, and service operators.

## Interfaces and Contracts

### Release and documentation contracts

- Build and maintain Action source under `actions/perfine` in `delinoio/apps`. Publish built Action artifacts and required runtime files to `https://github.com/delinoio/perfine-action`.
- Create release tags `vX.Y.Z`. Generated workflow references the exact verified distribution commit SHA. Do not provide moving major-version tags. Preserve repository rules that generated directories named `dist` are untracked; use a distribution layout consistent with that rule.
- Publish npm entry package `perfine`, its four platform packages, and standalone archives for all four targets with checksums in GitHub Releases. Keep packaging/build evidence separate from actual installation acceptance.
- Follow repository licensing: server-owned source/distributions FSL-1.1-ALv2; other repository-owned source/distributions Apache-2.0; retain third-party notices.
- Before runtime implementation, add `docs/project-perfine.md` and owning domain contracts, update the documentation catalog, define canonical ownership paths, and update scoped instructions only for ownership/procedure changes. Keep public usage/API/CLI documentation separate from internal contracts. Document adapter extension, policy configuration, setup, distribution, compatibility, consent, limitations, and troubleshooting. Do not insert issue/PR identifiers into repository docs.

Generated workflows follow [Cli contract](cmds-perfine-cli-contract.md): `init --github` puts required configuration in one manual `.github/workflows/perfine.yml`, pins the exact verified distributed Action commit, requires missing workload code separately, and confirms overwrites. No default push/PR optimization or publication. Hosting configuration, limits, telemetry, alerting, and runbooks follow [Api contract](servers-perfine-api-contract.md).

## Storage

Keep source/build inputs in the owning monorepo paths. Publish built Action artifacts and required runtime files in a distribution layout without tracked generated `dist` directories. Retain original raw evidence locally/on GitHub when it cannot be uploaded. Package binaries, checksums, licenses, and required notices together as applicable.

## Security

CI uploads use GitHub Actions OIDC with repository/execution scope validation. AI authentication stays in user repository Actions with GitHub Secrets and officially supported unattended paths; [Adapters contract](crates-perfine-adapters-contract.md) governs eligibility and credential separation. Publication is explicitly approved and draft-only under [Cli contract](cmds-perfine-cli-contract.md).

## Logging

Use structured request/experiment correlation and redact secrets/raw evidence. Distinguish packaging success from actual installation or live account acceptance.

## Build and Test

Provide automated npm contents, standalone binary/checksum, Action runtime/distribution layout, immutable SHA, `vX.Y.Z` tag, no-moving-major-tag, and third-party notice checks. Run `pnpm test` and `pnpm build` in the implemented Action/package directories, plus root `cargo test` after Rust changes. Remove generated `dist` directories from the final worktree. Release preparation must install the actual npm entry archive and its exact matching platform archive offline into a disposable directory on each target and extract that target’s standalone archive. Verify checksums, notices and provenance, run both executables with `--version`, and require the structured `doctor` missing-configuration diagnostic. This smoke check does not establish profiler readiness or live registry installation.

Full Docker workflows and live npm-install/GitHub/App/OIDC/AI checks are not a first-release blocker and remain unperformed unless actually executed. This exception does not waive package-name and publishing-authority verification or the required native package/executable checks for all four targets. Record minimum-OS validation separately; newer runners do not establish the minimum, and Windows Server does not establish Windows 11 compatibility.

Automated verification is the first-release gate. Follow the acceptance criteria and test scenarios in the [project index](project-perfine.md). Record source revision, commands, results, and unperformed checks in PR/CI evidence. Fixtures, mocked integrations, builds, and packaging do not establish live or native acceptance.

## Dependencies and Integrations

[Cli contract](cmds-perfine-cli-contract.md), [Adapters contract](crates-perfine-adapters-contract.md), [Api contract](servers-perfine-api-contract.md), [environment ownership](repository-environment-contract.md), and [license contract](repository-license-contract.md).

## Change Triggers

Update the project index and affected owning contracts together when ownership, interfaces, evidence, or policy changes. Update scoped instructions only for ownership or development-procedure changes; follow the [instruction-update policy](README.md#instruction-update-policy).

## References

- [Perfine project](project-perfine.md)
- [Repository defaults](repository-defaults.md)
- [Repository workflow](repository-workflow-contract.md)
- [Repository license](repository-license-contract.md)

## Concrete distribution interface

All release components use version `0.1.0` until a coordinated version change. The Rust package and executable are both `perfine`. The npm entry package selects the platform package for the actual host OS and CPU. Its `optionalDependencies` must use exact versions equal to the entry package version:

| Host           | Platform package                 |
| -------------- | -------------------------------- |
| Linux x86_64   | `@delinoio/perfine-linux-x64`    |
| macOS x86_64   | `@delinoio/perfine-darwin-x64`   |
| macOS ARM64    | `@delinoio/perfine-darwin-arm64` |
| Windows x86_64 | `@delinoio/perfine-win32-x64`    |

Each platform package contains its executable, binary checksum, source provenance, Apache-2.0 license and third-party notices. The launcher verifies the executable checksum before every launch and preserves arguments, streams, exit behavior and supported cancellation semantics. Unsupported hosts, missing optional packages, mismatched versions/architectures, corrupt checksums and failed startup must fail explicitly with recovery guidance. Never fall back to a runtime download or source compilation. Offline installation checks must provide the required platform package locally.

Verify availability and publishing authority for all approved npm names before publication. A registry 404 does not prove authority or reserve a name; an authentication failure cannot establish authority. If the names cannot be secured, block publication and obtain a new naming decision. Do not silently rename packages or treat a preparation build as registry acceptance.

Before checksums and native smoke tests, normalize the npm entry archive’s launcher mode to `0755` and its gzip OS marker to the platform-independent value. Preserve the other tar contents and retain the exact cross-target archive equality gate. This compensates for npm recording the launcher as `0644` on Windows despite the staged executable mode.

Archive creation and smoke extraction use relative archive paths from the release directory so GNU tar does not interpret Windows drive letters as remote hosts. Standalone archives for all four targets retain both archive-level and binary checksums, provenance, licenses and notices. macOS distribution signing/notarization and Windows Authenticode are outside scope. Runtime-required signing mechanics do not verify publisher identity. Document Gatekeeper, SmartScreen and organization-policy restrictions without recommending global security bypasses. Linux retains its GNU glibc 2.39 baseline; other minimum versions follow the project host policy.

`packages/perfine/scripts/build.mjs` stages the npm entry, selected platform package, standalone and Linux-only Action layouts under ignored `packages/perfine/dist/release`. The default input is `target/release/perfine`; `PERFINE_TARGET` selects an approved Rust target triple and defaults to `x86_64-unknown-linux-gnu`. When set, the default input becomes `target/<target>/release/perfine` (`perfine.exe` on Windows). `PERFINE_BINARY`, `PERFINE_NOTICES` and `PERFINE_OUTPUT` provide explicit packaging inputs for fixture tests or release automation. Real release builds obtain dependency license texts from the locked Cargo dependency graph. Default builds regenerate notices on every invocation; an existing generated notice file is not a cache. Missing license texts fail preparation unless a reviewed supplement in `packages/perfine/license-supplements/manifest.json` matches the exact crate name, version, registry source, repository, license expression and crate VCS revision. Supplements preserve the original upstream text, immutable source URL and SHA-256 checksum; changed or unknown dependencies fail closed. Remove a supplement when an upgraded crate ships its license texts. Ubuntu release builds install `protobuf-compiler` and `libprotobuf-dev`, then verify `protoc` and the required well-known proto includes before building the CLI. Fixtures must label their substitute binaries and notices as fixtures. Every layout includes the root Apache license, notice and source provenance. Native layouts also include target-specific dependency license texts and the binary checksum. The target-independent npm entry contains only the launcher and a notice directing users to the platform package for native dependency licenses. Every standalone archive and npm tarball also has a release-level checksum. Action output includes `action.cjs`, `action.yml`, `ACTION_NOTICES` and `runtime/`, with no tracked `dist` directory. The monorepo tracks Action source and bundled dependency notices. `actions/perfine/action.cjs` is ignored generated output, built before Action tests and CLI packaging. Only the distribution repository receives the built Action bundle; the Rust binary is generated only for distribution.

### Action and CLI integration

The Action accepts `command: measure`, an explicit UUID v7 `experiment`, and complete v1 YAML in `config`. It writes the exact YAML bytes to a private temporary file and invokes:

```text
perfine --repository <checkout> --config <private-file> --json measure --experiment <uuid-v7>
```

`config-sha256` is the exact YAML checksum. The Rust CLI validates required limits, workload, tools, immutable conditions and Docker isolation. The Action supplies a fresh home and an environment allowlist; it does not pass runner GitHub, OIDC or AI credentials to the CLI. The workflow must check out the declared source with `persist-credentials: false`, provision the trusted digest-pinned images and workload dependencies, and supply the experiment identifier. The generated manual workflow remains owned by the CLI. No Action command publishes candidates.

The current adapter accepts only local authentication files. That implementation does not establish support for Codex API-key authentication or Claude setup-token authentication inside the agent container. Cloud `optimize` and `resume` fail closed with the local CLI alternative until the adapter implements and tests the official unattended route. Do not substitute an exported local cached login or invent an auth-file format. Any later enabling change must preserve the Docker adapter's isolated agent role, candidate environment allowlist and mounts, and independent upload/publication roles.

### Separate OIDC upload stage

Use a fresh upload job with `id-token: write`, no AI/publication secrets and no candidate execution. The upload Action accepts `command: upload`, `experiment`, `envelope`, `artifacts`, `space`, `connection` and a stable UUID v7 `request-id`. The envelope contains the v1 Connect `specification` and `manifest`; the artifact directory contains regular files named by their SHA-256. The Action checks declared hashes and sizes before requesting credentials. It rejects source-bearing evidence and requires explicit secret screening. Upload never deletes local evidence.

Request an Actions OIDC token for audience `https://perfine.api.delino.io`. Check repository name/ID, run ID/attempt, source SHA, workflow SHA/ref and manual event against the runner context before transfer. Send the token only to the fixed Perfine endpoint using `Authorization: Actions <token>` with Connect JSON `BeginUpload` and `CompleteUpload`. Include the exact `githubExecution` binding. Token parsing on the runner is a consistency check; the server must independently verify its signature, issuer, audience, expiry, live repository connection, actor authority and execution binding. The two RPCs use the same UUID request ID within their separate method scopes; do not append a suffix. Signed storage PUT requests receive only permitted storage headers, never the OIDC token. A required `content-length` must equal the verified artifact byte count; duplicate header names are rejected. An incomplete completion response fails the stage and retains evidence for manual resend. This path does not provide raw-source consent management or export AI credentials to the service.

### Release preparation and exact commit verification

The reusable `deploy-perfine.yml` workflow is called by the main-only `release.yml` entry point. Changesets select the distribution and its version; preparation verifies the release commit belongs to main and matches core, CLI, Action, npm entry and all platform package versions. The `perfine@vX.Y.Z` success tag is created only after component deployment succeeds. All four targets must prepare and pass native actual-package installation and standalone executable checks before any publication. The owning release matrix and separate validation records follow the [workflow contract](repository-workflow-contract.md#perfine-release-validation). Its default `publish: false` builds and preserves reviewable artifacts without registry or repository publication. The unified release dispatch passes `publish: true` for a selected distribution, with publication gated by the repository-wide protected `release-publish` environment. Configure required reviewers, main-only deployment restrictions, `PERFINE_DISTRIBUTION_TOKEN` with contents-write authority only for the existing `delinoio/perfine-action` repository, and `PERFINE_NPM_TOKEN` with verified publishing authority for the entry package and all four approved platform packages. The owning job uses the source repository token for its GitHub Release. Do not create the external repository implicitly.

Action publication creates only an exact `vX.Y.Z` tag. It never updates a distribution branch, creates a moving major tag, force-pushes or replaces a tag. On retry, an existing tag must match every staged byte and source provenance. A different candidate requires a new version. After pushing, fetch the tag from the fixed distribution repository and verify its exact commit, layout, notices, binary checksum and source provenance. Return the verified SHA in `action-release.json` and the workflow's `action-commit` output. Generated workflows must use that SHA, never a source-monorepo SHA or a placeholder.

The independent verification command is:

```text
PERFINE_RELEASE_TAG=vX.Y.Z PERFINE_ACTION_COMMIT=<exact-distribution-sha> PERFINE_SOURCE_COMMIT=<exact-source-sha> pnpm --dir packages/perfine release:verify
```

This command performs read-only fetches from the official Action repository and returns the verified record. It cannot manufacture a release SHA. Registry and GitHub Release publication do not overwrite existing versions/assets. After a partial publication failure, inspect which immutable artifacts exist before retrying; reconcile the entry package, each platform package and GitHub outcomes manually instead of replacing them. Publish the entry package only after its exact platform dependencies are available and verified; never expose an entry release with missing required target packages. Packaging fixtures do not establish npm installation, native profiler, live OIDC, provider-account or release acceptance.
