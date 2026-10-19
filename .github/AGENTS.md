# .github working instructions

- Follow the root instruction-update policy and the nearest parent instructions.
- This file covers `.github/` and its descendants unless a more specific instruction file applies.
- Read the owning contracts before changing workflows, including cross-domain consumers.
- Resolve affected project selection before runner allocation. Keep macOS and iOS runners out of PR, main-push and affected manual CI; retain the separate owning mobile release gates.
- FixKey release tag pushes must resolve to a source commit on main before protected signing jobs or native runners are allocated. Keep manual recovery on main with an existing immutable tag.
- Preserve main-only mobile dispatch, the protected release environment, and opt-in test-channel uploads.
- Follow the release preparation and project publication procedures in the repository workflow contract when changing release automation.

## Owning contracts

- [Repository workflow](../docs/repository-workflow-contract.md)
- [DeliTools application](../docs/apps-delitools-foundation.md)
- [Environment ownership](../docs/repository-environment-contract.md)

- [FixKey desktop and gated release](../docs/apps-fixkey-desktop-contract.md)
