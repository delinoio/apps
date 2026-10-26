---
"@apps/lane": patch
---

Bind a conflict resolution session to the head versions reviewed when it began. Submit those
head IDs, not the refreshed task heads, block a stale session and require an explicit
"Review current versions" action before a retained edited resolution can be reused.

<!-- public-changelog:start -->

When a conflicting task changes while you are resolving it, Lane keeps your edited
resolution visible but no longer saves it over the newer versions. Review the current
versions, then choose or reuse your resolution to save it.

<!-- public-changelog:end -->
