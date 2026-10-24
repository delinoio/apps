# .github working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `.github/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts before changing workflows, including cross-domain consumers.
- Keep Rust server Clippy and tests in separate jobs with the same affected-service matrix and independent validation artifacts. Do not install Go or run Go compatibility checks in those jobs. Ordinary CI omits OCI tests and image builds; retain owning release image publication.
- Run all formatting checks without change detection or a dependency on the changes job.
- Protocol CI validates Buf lint, generated-binding freshness and consumer builds without running Buf breaking comparisons. Keep the local comparison command available under the repository workflow contract.
- Resolve affected project selection before runner allocation. Keep macOS and iOS runners out of PR, main-push and affected manual CI; retain the separate owning mobile release gates.
- Preserve main-only mobile dispatch, the protected release environment, and opt-in test-channel uploads.
- Keep Delino web publication in the shared release workflow and shared production environment; follow the application membership and environment ownership in the repository workflow contract.
- Follow the release preparation and project publication procedures in the repository workflow contract when changing release automation.

## Owning contracts

- [Repository workflow](../docs/repository-workflow-contract.md)
- [DeliTools application](../docs/apps-delitools-foundation.md)
- [Environment ownership](../docs/repository-environment-contract.md)
