---
"@servers/lane-api": patch
---

Keep rejected sync bookkeeping under the authorized account lock so clearing and revocation cannot race old pushes.

<!-- public-changelog:start -->

Account clearing now prevents rejected synchronization items from reappearing after the operation completes.
<!-- public-changelog:end -->
