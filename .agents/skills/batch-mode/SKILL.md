---
name: batch-mode
description: Select the Linear issues to process in a batch and apply additional CI constraints only when a human explicitly invokes $batch-mode or requests Batch Mode. Do not activate automatically for batch work or CI tasks.
---

# Batch Mode

Apply these additional instructions to the current task only when a human explicitly invokes this skill.

## Batch Issue Selection

The batch contains every Linear issue that matches both filters:

- Team: `DELI` (Mono).
- Status: `Todo`.

Do not add issues from other teams or statuses, such as `Backlog`, `In Progress` or `In Review`, to the batch.

1. Query Linear with `list_issues` (Linear MCP server), `team: "DELI"` and `state: "Todo"`. Request the `id`, `title`, `priority`, `labels`, `assignee` and `url` fields.
2. When `hasNextPage` is `true`, fetch the next page with the returned `cursor` until all pages are read. Do not assume that the first page is complete.
3. If the `Todo` status cannot be resolved, run `list_issue_statuses` for the `DELI` team and use the status named `Todo`. Do not substitute another status.
4. Report the batch as a list. For each issue, give the identifier (for example `DELI-123`), the title, the priority and the URL. If no issue matches, report that the batch is empty.

Use the issue identifiers in this list for the `Closes DELI-<number>` and `Refs DELI-<number>` lines of each PR. Follow [the issue tracker contract](../../../docs/repository-issue-tracker-contract.md).

## CI Constraints

- Do not start manual CI or trigger `workflow_dispatch`.
- Use the existing PR's automatic CI.
- Retrying a failed CI run is allowed.
- Never modify `.github/workflows/CI.yml`.
- Prioritize issues assigned to Batch group 1 over issues assigned to Batch group 2.

This skill selects the batch issues and adds instructions only. It does not start batch work or create a PR.
