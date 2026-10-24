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

Provide automated npm contents, standalone binary/checksum, Action runtime/distribution layout, immutable SHA, `vX.Y.Z` tag, no-moving-major-tag, and third-party notice checks. Run `pnpm test` and `pnpm build` in the implemented Action/package directories, plus root `cargo test` after Rust changes. Remove generated `dist` directories from the final worktree. Live npm/GitHub/App/OIDC/AI checks are not a first-release blocker and remain unperformed unless actually executed.

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
