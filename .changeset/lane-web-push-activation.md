---
"@apps/lane": patch
---

Wait for the notification worker registration to become active before creating a Web Push
subscription, so first-time enablement no longer fails with an invalid-state error. Add
activation-order regression tests and document the web enablement ordering.

<!-- public-changelog:start -->

Turning on notifications in Lane on the web now works the first time, without needing to try again.

<!-- public-changelog:end -->
