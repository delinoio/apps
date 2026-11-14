# GitHub workflow instructions

- Follow root `AGENTS.md` and [the workflow contract](../docs/repository-workflow-contract.md).
- Keep PRs, main pushes and manual validation on non-macOS runners.
- Connect every new PR validation job to `ci-result` through its `needs` list. Keep `ci-result` running after dependency failures and cancellations so it reports the aggregate result.
- Use the `changes` job outputs to select affected validation. Update the planner inputs when shared validation dependencies change, and include `changes` in `ci-result.needs`.
- Manual validation uses the selected revision comparison. Do not add a full manual mode.
- Add release workflows only with an owning project contract and explicit credential boundaries.
- Follow [the deployment contract](../docs/repository-deployment-contract.md) for deployment workflows. Use only the `static-sites` and `servers` GitHub environments, keep deployment credentials out of pull request workflows, and express component dependencies through job `needs` so dependents deploy only after their dependencies succeed or are skipped.
- Register each new deployable component in `deploy.yml`, `scripts/ci/deploy-plan.mjs` and the deployment contract in the same change.
