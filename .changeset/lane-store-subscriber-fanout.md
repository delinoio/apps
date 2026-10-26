---
"@apps/lane": patch
---

Track the accepted store-change sequence per NativeAdapter subscription instead of in shared adapter state, so every active subscriber receives each newer `lane.storeChanged` event once. Rebase each subscription when the store generation or data space changes. The mounted URL connection settings and capture composer rule cache now refresh after external changes.

<!-- public-changelog:start -->

Lane on desktop now refreshes the URL connection list and the capture composer's URL rules after another process changes them, without discarding unsaved edits or reassigning an already matched draft.

<!-- public-changelog:end -->
