# GitHub workflow instructions

- Follow root `AGENTS.md` and [the workflow contract](../docs/repository-workflow-contract.md).
- Keep PRs, main pushes and manual validation on non-macOS runners.
- Connect every new PR validation job to `ci-result` through its `needs` list. Keep `ci-result` running after dependency failures and cancellations so it reports the aggregate result.
- Use the `changes` job outputs to select affected validation. Update the planner inputs when shared validation dependencies change, and include `changes` in `ci-result.needs`.
- Manual validation uses the selected revision comparison. Do not add a full manual mode.
- Do not check out the private `servers/` submodule or reference server secrets in public workflows. Server CI lives in the `delinoio/servers` repository.
- Add release workflows only with an owning project contract and explicit credential boundaries.
