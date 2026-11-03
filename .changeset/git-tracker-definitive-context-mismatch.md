---
"@servers/git-tracker-api": patch
---

Return `failed_precondition` instead of `unavailable` when GitHub has definitively answered that the upload or completion context is wrong: a complete, consistent pull request traversal that excludes the commit, a pull request base ref that differs from the context branch, and a push commit that is not on the first-parent path of the branch head. These faults keep the application source, so they no longer increment `git_tracker_events_total{kind="github",category="failure"}`, and the Action, which only retries transient statuses, stops retrying them. Deadline expiry, GraphQL errors, inconsistent or incomplete pages, snapshot changes and transport or REST failures still return `unavailable` with the GitHub source and the counter increment.

<!-- public-changelog:start -->

Uploads and completions whose commit is not part of the pull request, whose pull request targets a different base branch, or whose push commit is not on the branch now fail immediately with a clear `failed_precondition` message instead of being retried as a temporary outage.

<!-- public-changelog:end -->
