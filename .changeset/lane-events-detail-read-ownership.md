---
"@apps/lane": patch
---

Give Events detail reads an ownership generation through `EventDetailReader`. The newest
inspection, Close, Escape, Open linked task, leaving the page and adapter replacement
end the ownership of older `GetEvent` reads, so a late response or failure cannot
replace the accepted event, reopen a dismissed dialog or publish an obsolete error.
Adds model regression tests and a headless-browser fixture check
(`tests/events-detail-browser.mjs`) with controlled reply ordering.

<!-- public-changelog:start -->

Inspecting an event in Events now always shows the event you chose last. A slow
earlier inspection no longer replaces the details, reopens a dialog you closed or shows
an outdated error.

<!-- public-changelog:end -->
