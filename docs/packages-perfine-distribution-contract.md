# Perfine Action and CLI distribution

## Scope

`actions/perfine` owns Action source and build inputs. `packages/perfine` owns npm package `perfine` and CLI packaging. Publish Action output to `delinoio/perfine-action`; the source remains in this monorepo.

## Runtime and Language

The executable is the Rust CLI from `cmds/perfine`. Node.js packaging and Action tooling follow the root runtime selector and pinned pnpm toolchain. Official standalone execution targets Linux x86_64. Keep release distributions outside tracked directories named `dist`.

## Users and Operators

Local CLI users, repository administrators generating workflows, Action maintainers, release operators, and service operators.

## Interfaces and Contracts

### Release and documentation contracts

- Build and maintain Action source under `actions/perfine` in `delinoio/apps`. Publish built Action artifacts and required runtime files to `https://github.com/delinoio/perfine-action`.
- Create release tags `vX.Y.Z`. Generated workflow references the exact verified distribution commit SHA. Do not provide moving major-version tags. Preserve repository rules that generated directories named `dist` are untracked; use a distribution layout consistent with that rule.
- Publish npm package `perfine` and Linux x86_64 standalone CLI binaries with checksums in GitHub Releases. Keep packaging/build evidence separate from actual installation acceptance.
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

Provide automated npm contents, standalone binary/checksum, Action runtime/distribution layout, immutable SHA, `vX.Y.Z` tag, no-moving-major-tag, and third-party notice checks. Run `pnpm test` and `pnpm build` in the implemented Action/package directories, plus root `cargo test` after Rust changes. Remove generated `dist` directories from the final worktree. CI and release preparation install the actual npm archive offline into a disposable directory and extract the standalone archive. Verify checksums, notices and provenance, run both executables with `--version`, and require the structured `doctor` missing-configuration diagnostic. This smoke check does not establish profiler readiness or live registry installation.

Live npm/GitHub/App/OIDC/AI checks are not a first-release blocker and remain unperformed unless actually executed.

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

All release components use version `0.1.0` until a coordinated version change. The Rust package and executable are both `perfine`. The npm launcher executes its bundled `runtime/perfine` after checking `runtime/SHA256SUMS`. It does not download binaries during installation. Release builds target Linux x86_64 GNU on Ubuntu 24.04; users need a compatible GNU runtime (glibc 2.39 or later). Other platforms fail with an explicit unsupported-platform error.

`packages/perfine/scripts/build.mjs` stages npm, standalone and Action layouts under ignored `packages/perfine/dist/release`. The default input is `target/release/perfine`; `PERFINE_BINARY`, `PERFINE_NOTICES` and `PERFINE_OUTPUT` provide explicit packaging inputs for fixture tests or release automation. Real release builds obtain dependency license texts from the locked Cargo dependency graph. Missing license texts fail preparation and require maintainer review. Fixtures must label their substitute binaries and notices as fixtures. Every layout includes the root Apache license and notice, dependency license texts, binary checksum and source provenance. The standalone tarball and npm tarball also have release-level checksums. Action output includes `action.cjs`, `action.yml`, `ACTION_NOTICES` and `runtime/`, with no tracked `dist` directory. The monorepo commits the Action bundle and its bundled dependency notices; the Rust binary is generated only for distribution.

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

The dedicated `release-perfine.yml` workflow runs only by manual dispatch from main. Its input `vX.Y.Z` selects an existing immutable source tag `perfine@vX.Y.Z`; preparation verifies main ancestry and matching core, CLI, Action and npm versions. Its default `publish: false` builds and preserves reviewable artifacts without registry or repository publication. Publication requires an explicit `publish: true` dispatch and the protected `perfine-production` environment. Configure required reviewers, main-only deployment restrictions, `PERFINE_DISTRIBUTION_TOKEN` with contents-write authority only for the existing `delinoio/perfine-action` repository, and `PERFINE_NPM_TOKEN` for the npm package. The owning job uses the source repository token for its GitHub Release. Do not create the external repository implicitly.

Action publication creates only an exact `vX.Y.Z` tag. It never updates a distribution branch, creates a moving major tag, force-pushes or replaces a tag. On retry, an existing tag must match every staged byte and source provenance. A different candidate requires a new version. After pushing, fetch the tag from the fixed distribution repository and verify its exact commit, layout, notices, binary checksum and source provenance. Return the verified SHA in `action-release.json` and the workflow's `action-commit` output. Generated workflows must use that SHA, never a source-monorepo SHA or a placeholder.

The independent verification command is:

```text
PERFINE_RELEASE_TAG=vX.Y.Z PERFINE_ACTION_COMMIT=<exact-distribution-sha> PERFINE_SOURCE_COMMIT=<exact-source-sha> pnpm --dir packages/perfine release:verify
```

This command performs read-only fetches from the official Action repository and returns the verified record. It cannot manufacture a release SHA. Registry and GitHub Release publication do not overwrite existing versions/assets. After a partial publication failure, inspect which immutable artifacts exist before retrying; reconcile npm/GitHub outcomes manually instead of replacing them. Packaging fixtures do not establish npm installation, native profiler, live OIDC, provider-account or release acceptance.
