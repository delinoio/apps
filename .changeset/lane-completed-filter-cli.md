---
"@delinoio/lane": minor
---

Add the typed completed-list predicate to the Lane CLI and local MCP. Evaluate the current exact terminal-state entry from immutable task ancestry before sorting and pagination.

<!-- public-changelog:start -->

Lane CLI and local MCP task lists can now filter completed tasks by a fixed UTC cutoff. CLI duration queries require `--completed-cutoff`; reuse the same value when loading another page.

<!-- public-changelog:end -->
