# Repository Issue Tracker Contract

## Scope

This contract defines where repository work is tracked and how pull requests reference tracked work. It applies to every project and domain in the repository.

## Tracker ownership

- The Linear team `DELI` (Mono) at https://linear.app/delinoio/team/DELI/ is the primary issue tracker. Track all development work there, including bugs, features, tasks, chores, investigations and follow-ups from reviews, QA and static analysis.
- GitHub Issues in this repository are for customer support only. Do not create GitHub issues for development work.
- Linear issue identifiers use the form `DELI-<number>`.

## Linear issue style

- Write Linear issue titles and explanatory body text in English. Preserve exact code, commands, paths, diagnostic messages, search queries, direct quotations and localized UI strings when their original text is required. Explain non-English evidence in English.
- Use issue titles in the format `<domain>: <description>`. `<domain>` uses stable lowercase identifiers from project and domain contracts, such as `tooling` or `docs`. `<description>` is concise and specific, and starts with a lowercase verb phrase when possible.
- Do not use bracket-style project prefixes such as `[project]`.
- Use these Markdown sections in this order: `## Summary`, `## Evidence`, `## Current Gap`, `## Proposed Scope`, `## Acceptance Criteria`, `## Test Scenarios`, `## Out of Scope`. Append `## Additional Notes` only when needed.
- When existing issue prose is not in English, translate it without changing requirements, evidence, references, attachments or issue state.

## Pull request references

Linear's GitHub integration links a pull request to a Linear issue when a magic word and the issue identifier appear in the pull request title or description. Magic words in pull request comments do not create links. See [Linear's GitHub integration documentation](https://linear.app/docs/github).

- For each fully resolved Linear issue, add a standalone `Closes DELI-<number>` line to the pull request description. Linear moves the issue to the team's merge status when the pull request merges.
- For each partially addressed or related Linear issue, add a standalone `Refs DELI-<number>` line instead. A `Refs` line links the issue without moving it to the merge status.
- Do not use both `Closes` and `Refs` for the same issue.
- Branch names may include the Linear issue identifier.

## Traceability

The documentation rule against issue and pull request identifiers in instruction files and documentation also applies to Linear identifiers. Record traceability, implementation status and validation results in pull requests, Linear issues and CI records.
