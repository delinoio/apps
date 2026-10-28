---
"@servers/lane-api": patch
---

Retire correlated synchronization failure records when a corrected immutable version is accepted, while preserving unrelated and legacy records.

<!-- public-changelog:start -->

Lane now clears a pending synchronization failure after the corrected change is accepted. Older failures without enough information remain until their original request succeeds or account data is cleared.

<!-- public-changelog:end -->
