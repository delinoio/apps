---
"@servers/git-tracker-api": patch
---

Bound the first-parent ancestry collection that runs for each push upload and first completion. The server now stores at most the nearest 1,000 ancestors, reads them through the paginated commit list (at most 12 pages, 100 commits each) into a parent map, and falls back to a single-commit request only for a first parent that those pages do not cover, instead of one GitHub request per ancestor to the repository root. The whole collection runs under a 20 second deadline whose expiry returns the existing GitHub-unavailable fault and persists nothing, and reaching the depth limit logs the `ancestry_truncated` event. Pull request uploads, explicit baselines, retransmissions and the `Git ancestry changed` check are unchanged.

<!-- public-changelog:start -->

Uploads from long-lived branches no longer slow down or time out while the server walks the whole commit history. Baselines are found among the nearest 1,000 first-parent commits, so a row whose nearest measured ancestor is older than that shows as comparison-unavailable.

<!-- public-changelog:end -->
