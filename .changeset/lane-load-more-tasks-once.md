---
"@apps/lane": patch
---

Serialize "Load more tasks" continuation loading per query generation. The control reserves
its cursor before calling the adapter, is disabled while pending, accepts a page only while
it owns the continuation and releases the lock on success or failure. A failed page keeps the
loaded rows and shows its error next to the control for explicit retry. Add controlled-adapter
App regression tests with a reusable App bundling harness.

<!-- public-changelog:start -->

Tapping "Load more tasks" repeatedly no longer shows the same tasks more than once. If loading
more tasks fails, the tasks already shown stay on screen and you can try again.

<!-- public-changelog:end -->
