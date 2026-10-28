---
"@apps/lane": patch
---

Bound iOS synchronization uploads, isolate oversized items, and keep dependent mutations held.

<!-- public-changelog:start -->

Oversized items no longer stop other Lane changes from syncing on iOS. Correct a blocked item and retry it to upload that item.
<!-- public-changelog:end -->
