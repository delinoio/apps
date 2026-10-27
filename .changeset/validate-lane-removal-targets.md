---
"@servers/lane-api": patch
---

Validate explicit task relationship removal targets in the account transaction and reject deleted targets with NOT_FOUND.

<!-- public-changelog:start -->

Task relationship edits now report an error when a removal names a deleted or invalid task.
<!-- public-changelog:end -->
