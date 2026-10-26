---
"@apps/lane": patch
---

Pass the App invalidation revision into the Events page and include it in the load
effect, so a delivered server-change notification rereads receivers and the first
event page for the active filters through the existing `EventsLoader` ownership.
Continuation restarts from the refreshed cursor, unsaved receiver input is kept, and
obsolete loads cannot publish. Adds model regression tests and a headless-browser
fixture check (`tests/events-browser.mjs`).

<!-- public-changelog:start -->

Events and webhook receivers now update on their own when new events arrive, an event
changes state or a receiver changes, without pressing Refresh events. Your filters and
any receiver you are editing stay as they are.

<!-- public-changelog:end -->
