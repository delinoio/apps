---
"@apps/lane": patch
---

Persist disabled web notification preferences through the server registration even when the browser
has no push subscription, so turning notifications off or choosing Generic content while Off is kept
after reload. A rejected save no longer reports success. Add OnlineLane and Settings regression tests
and document the web save ordering.

<!-- public-changelog:start -->

Turning off notifications, or choosing the generic notice while they are off, in Lane on the web is now saved and still applies after you reload.

<!-- public-changelog:end -->
