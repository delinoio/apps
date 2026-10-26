---
"@apps/lane": patch
---

Show a rejected task deletion inside the permanent-deletion confirmation of the Lane
web editor. The confirmation stays open with the existing failure message, keeps the
editor draft, sends no automatic retry and clears an earlier unrelated editor error
when it opens. A browser regression covers server, permission-denied and offline
failures, Close and Escape, and explicit retry at desktop and narrow widths.

<!-- public-changelog:start -->

Fixed task deletion failures being hidden behind the confirmation. The message now
appears in the "Permanently delete task?" dialog, where you can retry or close it
without losing your unsaved edits.

<!-- public-changelog:end -->
