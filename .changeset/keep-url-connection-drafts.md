---
"@apps/lane": patch
---

Keep URL connection and conflict-resolution drafts when an earlier mutation completes, and lock their submitted controls during pending requests.

<!-- public-changelog:start -->

URL connection fields stay locked while a save is pending. Changes accepted just before Save remain in the editor instead of being lost when the earlier save finishes.
<!-- public-changelog:end -->
