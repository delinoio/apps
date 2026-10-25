# GitHub Actions ownership

- Follow root instructions. This directory owns official client Actions.
- Git Tracker's Action is in `git-tracker`; read the [project](../docs/project-git-tracker.md) and [Action contract](../docs/packages-git-tracker-action-contract.md).
- Run `pnpm test` and `pnpm build` in the Action directory after changes. For Git Tracker, commit the standalone `action.cjs` bundle with source changes; do not create a tracked `dist` directory.
- Preserve imported dependency licenses in the Action's notices.

- Perfine ownership: [project](../docs/project-perfine.md); [distribution contract](../docs/packages-perfine-distribution-contract.md).

- Perfine generates its ignored `action.cjs` bundle before tests and packaging. Do not check this bundle into the source repository. Keep its dependency notices tracked.
