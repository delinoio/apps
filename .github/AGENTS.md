# .github working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `.github/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts before changing workflows, including cross-domain consumers.
- Keep Rust server Clippy and tests in separate jobs with the same affected-service matrix and independent validation artifacts. Do not install Go or run Go compatibility checks in those jobs. Ordinary CI omits server OCI tests and release image builds; disposable tool images for fixture validation are permitted; retain owning release image publication.
- Run all formatting checks without change detection or a dependency on the changes job.
- Protocol CI validates Buf lint, generated-binding freshness and consumer builds without running Buf breaking comparisons. Keep the local comparison command available under the repository workflow contract.
- Resolve affected project selection before runner allocation. Keep macOS and iOS runners out of PR, main-push and affected manual CI; retain the separate owning mobile release gates. The Perfine and Lane binary reusable workflows called by the main-only unified release may use macOS runners for their owning distribution gates only; follow the release sections of the repository workflow contract.
- Preserve main-only mobile dispatch, the protected release environment, and opt-in test-channel uploads.
- Keep every web app in the shared `delino-site-production` environment, native signing in repository-wide `release-signing`, and store/registry publication in repository-wide `release-publish`. Do not create app-specific release environments; follow the application membership and environment ownership in the repository workflow contract.
- Follow the release preparation and project publication procedures in the repository workflow contract when changing release automation.

## Owning contracts

- [Repository workflow](../docs/repository-workflow-contract.md)
- [DeliTools application](../docs/apps-delitools-foundation.md)
- [Environment ownership](../docs/repository-environment-contract.md)

- Lane CLI/distribution validation runs on Ubuntu in its affected job. Keep Lane API in the existing independent server Clippy/test matrices and require all selected Lane jobs in result aggregation; native acceptance remains separate.

- Server infrastructure, runtime rollout and monitoring configuration belong to private operator automation. Keep server source, migrations, safe examples and OCI packaging here. Public release automation accepts authenticated preparation/publication dispatches under the [workflow contract](../docs/repository-workflow-contract.md#project-release-preparation); never name private deployment repositories in public source or review records.
