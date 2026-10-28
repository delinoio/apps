---
"@servers/lane-api": patch
---

Canonicalize task planning sets before conflict-resolution and event-completion receipt comparisons.

<!-- public-changelog:start -->

Lane API retries of task conflict resolution and webhook event completion now replay the original result when planning IDs are reordered or repeated.

<!-- public-changelog:end -->
